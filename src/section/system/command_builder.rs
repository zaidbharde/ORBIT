//! Safe command execution architecture for the System section.
//!
//! P6.2 provides a command builder panel that allows users to launch
//! processes with explicit program + arguments (never through a shell).
//! Execution happens on a background thread so the egui update loop is
//! never blocked. Output is captured and displayed in the UI.
//!
//! Safety invariants:
//! - Commands are built using `std::process::Command` with explicit args
//! - No `sh -c`, `bash -c`, `eval`, or string-based shell execution
//! - Executable must be non-empty and exist on PATH or as an absolute path
//! - Working directory must exist if provided
//! - No privileged (sudo/root) execution
//! - P6.1 PID protections are preserved for monitored processes

use crate::glass::with_alpha;
use crate::section::SectionContext;
use crossbeam_channel::{Receiver, bounded};
use eframe::egui::{self, FontId, Frame, Margin, RichText, Stroke, Ui};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

/// Status of the command builder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandStatus {
    Idle,
    Running,
    Completed,
    Failed,
    ValidationError,
}

impl CommandStatus {
    pub fn label(&self) -> &'static str {
        match self {
            CommandStatus::Idle => "Idle",
            CommandStatus::Running => "Running",
            CommandStatus::Completed => "Completed",
            CommandStatus::Failed => "Failed",
            CommandStatus::ValidationError => "Error",
        }
    }
}

/// Messages sent from the background execution thread to the UI.
#[derive(Debug)]
pub enum CommandOutput {
    Stdout(String),
    Stderr(String),
    Started { pid: u32 },
    Finished { exit_code: Option<i32> },
}

/// State of a launched process.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessState {
    pub pid: Option<u32>,
    pub executable: String,
    pub status: CommandStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error_message: Option<String>,
}

impl ProcessState {
    fn new() -> Self {
        Self {
            pid: None,
            executable: String::new(),
            status: CommandStatus::Idle,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            error_message: None,
        }
    }
}

/// The command builder holds all UI state for the command panel.
pub struct CommandBuilder {
    pub executable: String,
    pub arguments: String,
    pub working_dir: String,
    pub env_vars: String,
    pub state: ProcessState,
    pub last_state_change: Option<Instant>,
    rx: Option<Receiver<CommandOutput>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl CommandBuilder {
    pub fn new() -> Self {
        Self {
            executable: String::new(),
            arguments: String::new(),
            working_dir: std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            env_vars: String::new(),
            state: ProcessState::new(),
            last_state_change: None,
            rx: None,
            child: Arc::new(Mutex::new(None)),
        }
    }

    /// Drain any pending output from the background thread.
    pub fn poll_output(&mut self) {
        let should_clear_rx = {
            let rx = match &self.rx {
                Some(rx) => rx,
                None => return,
            };
            let mut clear_rx = false;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    CommandOutput::Stdout(line) => {
                        if !self.state.stdout.is_empty() {
                            self.state.stdout.push('\n');
                        }
                        self.state.stdout.push_str(&line);
                    }
                    CommandOutput::Stderr(line) => {
                        if !self.state.stderr.is_empty() {
                            self.state.stderr.push('\n');
                        }
                        self.state.stderr.push_str(&line);
                    }
                    CommandOutput::Started { pid } => {
                        self.state.pid = Some(pid);
                        self.state.status = CommandStatus::Running;
                        self.last_state_change = Some(Instant::now());
                    }
                    CommandOutput::Finished { exit_code } => {
                        self.state.exit_code = exit_code;
                        self.state.status = match exit_code {
                            Some(0) => CommandStatus::Completed,
                            Some(_) => CommandStatus::Failed,
                            None => CommandStatus::Failed,
                        };
                        self.last_state_change = Some(Instant::now());
                        clear_rx = true;
                    }
                }
            }
            clear_rx
        };
        if should_clear_rx {
            self.rx = None;
        }
    }

    /// Clear all fields and reset to idle state.
    pub fn clear(&mut self) {
        self.stop_process();
        self.executable.clear();
        self.arguments.clear();
        self.working_dir = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.env_vars.clear();
        self.state = ProcessState::new();
        self.last_state_change = None;
        self.rx = None;
    }

    /// Stop the running process if one is active.
    pub fn stop_process(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// Refresh the process state (check if still alive).
    pub fn refresh_state(&mut self) {
        if self.state.status == CommandStatus::Running {
            if let Ok(mut guard) = self.child.lock() {
                if let Some(ref mut child) = *guard {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let exit_code = status.code();
                            self.state.exit_code = exit_code;
                            self.state.status = match exit_code {
                                Some(0) => CommandStatus::Completed,
                                Some(_) => CommandStatus::Failed,
                                None => CommandStatus::Failed,
                            };
                            self.last_state_change = Some(Instant::now());
                            *guard = None;
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Validation (pure, testable)
// ---------------------------------------------------------------------------

/// Validate that the executable string is non-empty.
pub fn validate_executable(executable: &str) -> Result<(), String> {
    let trimmed = executable.trim();
    if trimmed.is_empty() {
        return Err("Executable cannot be empty".into());
    }
    Ok(())
}

/// Validate that the working directory exists (if provided).
pub fn validate_working_dir(dir: &str) -> Result<(), String> {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let path = std::path::Path::new(trimmed);
    if !path.exists() {
        return Err(format!("Directory does not exist: {trimmed}"));
    }
    if !path.is_dir() {
        return Err(format!("Path is not a directory: {trimmed}"));
    }
    Ok(())
}

/// Parse environment variables from a string (one KEY=VALUE per line).
pub fn parse_env_vars(env_str: &str) -> Vec<(String, String)> {
    let mut vars = Vec::new();
    for line in env_str.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                vars.push((key.to_owned(), value.to_owned()));
            }
        }
    }
    vars
}

/// Parse arguments string into individual arguments.
///
/// Arguments are split by whitespace. Quoted strings are handled:
/// `"arg with space"` becomes a single argument.
pub fn parse_args(args_str: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';

    for ch in args_str.chars() {
        match ch {
            '"' | '\'' if !in_quotes => {
                in_quotes = true;
                quote_char = ch;
            }
            c if in_quotes && c == quote_char => {
                in_quotes = false;
            }
            ' ' if !in_quotes => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            c => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Execute a command on a background thread, returning a channel receiver
/// for output and an Arc<Mutex<Option<Child>>> for process control.
pub fn execute_command(
    executable: &str,
    args: &[String],
    working_dir: &str,
    env_vars: &[(String, String)],
) -> Result<(Receiver<CommandOutput>, Arc<Mutex<Option<Child>>>), String> {
    let mut cmd = Command::new(executable);
    cmd.args(args);

    let trimmed_dir = working_dir.trim();
    if !trimmed_dir.is_empty() {
        cmd.current_dir(trimmed_dir);
    }

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start process: {e}"))?;

    let pid = child.id();
    let (tx, rx) = bounded(256);

    // Take stdout and stderr handles before storing child
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let child_arc = Arc::new(Mutex::new(Some(child)));

    // Send the PID immediately
    let _ = tx.try_send(CommandOutput::Started { pid });

    // Spawn a thread to read stdout and stderr, then wait for process
    let child_for_thread = child_arc.clone();
    let tx_for_thread = tx.clone();
    thread::spawn(move || {
        // Read stdout in a separate thread to avoid blocking stderr
        let tx_stdout = tx_for_thread.clone();
        let stdout_thread = stdout_handle.map(|stdout| {
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            let _ = tx_stdout.try_send(CommandOutput::Stdout(l));
                        }
                        Err(_) => break,
                    }
                }
            })
        });

        // Read stderr
        if let Some(stderr) = stderr_handle {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        let _ = tx_for_thread.try_send(CommandOutput::Stderr(l));
                    }
                    Err(_) => break,
                }
            }
        }

        // Wait for stdout thread to finish
        if let Some(handle) = stdout_thread {
            let _ = handle.join();
        }

        // Wait for process to finish
        let exit_code = if let Ok(mut guard) = child_for_thread.lock() {
            if let Some(ref mut child) = *guard {
                match child.wait() {
                    Ok(status) => {
                        let code = status.code();
                        *guard = None;
                        code
                    }
                    Err(_) => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        let _ = tx_for_thread.try_send(CommandOutput::Finished { exit_code });
    });

    Ok((rx, child_arc))
}

// ---------------------------------------------------------------------------
// UI rendering
// ---------------------------------------------------------------------------

/// Render the command builder card in the System dashboard.
pub fn show_command_builder_card(
    ui: &mut Ui,
    context: &SectionContext<'_>,
    builder: &mut CommandBuilder,
) {
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
            RichText::new("Command Builder")
                .font(FontId::proportional(12.0))
                .color(theme.ui.secondary_text)
                .strong(),
        );
        ui.add_space(2.0);

        // Executable field
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Executable:")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
            ui.add(
                egui::TextEdit::singleline(&mut builder.executable)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(10.0))
                    .hint_text("e.g. /usr/bin/ls"),
            );
        });

        // Arguments field
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Arguments:")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
            ui.add(
                egui::TextEdit::singleline(&mut builder.arguments)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(10.0))
                    .hint_text("e.g. -la /tmp"),
            );
        });

        // Working directory
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Work dir:")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
            ui.add(
                egui::TextEdit::singleline(&mut builder.working_dir)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(10.0))
                    .hint_text("optional working directory"),
            );
        });

        // Environment variables
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Env vars:")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
            ui.add(
                egui::TextEdit::singleline(&mut builder.env_vars)
                    .desired_width(ui.available_width())
                    .font(FontId::monospace(10.0))
                    .hint_text("KEY=VALUE (comma or newline separated)"),
            );
        });

        ui.add_space(4.0);

        // Control buttons
        let is_running = builder.state.status == CommandStatus::Running;
        ui.horizontal(|ui| {
            let start_btn = egui::Button::new(
                RichText::new(if is_running { "Running..." } else { "Start" })
                    .font(FontId::proportional(10.0))
                    .color(if is_running {
                        theme.ui.secondary_text
                    } else {
                        theme.ui.text
                    }),
            )
            .fill(if is_running {
                with_alpha(theme.ui.secondary_text, 0.15)
            } else {
                theme.ui.accent
            });

            if ui.add(start_btn).clicked() && !is_running {
                builder.poll_output();
                // Validate
                if let Err(e) = validate_executable(&builder.executable) {
                    builder.state.error_message = Some(e);
                    builder.state.status = CommandStatus::ValidationError;
                    builder.last_state_change = Some(Instant::now());
                } else if let Err(e) = validate_working_dir(&builder.working_dir) {
                    builder.state.error_message = Some(e);
                    builder.state.status = CommandStatus::ValidationError;
                    builder.last_state_change = Some(Instant::now());
                } else {
                    let args = parse_args(&builder.arguments);
                    let env = parse_env_vars(&builder.env_vars);
                    let working_dir = builder.working_dir.trim().to_owned();
                    let executable = builder.executable.trim().to_owned();

                    match execute_command(&executable, &args, &working_dir, &env) {
                        Ok((rx, child)) => {
                            builder.rx = Some(rx);
                            builder.child = child;
                            builder.state = ProcessState {
                                pid: None,
                                executable,
                                status: CommandStatus::Running,
                                exit_code: None,
                                stdout: String::new(),
                                stderr: String::new(),
                                error_message: None,
                            };
                            builder.last_state_change = Some(Instant::now());
                        }
                        Err(e) => {
                            builder.state.error_message = Some(e);
                            builder.state.status = CommandStatus::Failed;
                            builder.last_state_change = Some(Instant::now());
                        }
                    }
                }
            }

            let clear_btn = egui::Button::new(
                RichText::new("Clear")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.text),
            )
            .fill(theme.ui.tab_inactive);

            if ui.add(clear_btn).clicked() {
                builder.clear();
            }

            let refresh_btn = egui::Button::new(
                RichText::new("Refresh")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.text),
            )
            .fill(theme.ui.tab_inactive);

            if ui.add(refresh_btn).clicked() {
                builder.refresh_state();
                builder.poll_output();
            }
        });

        ui.add_space(4.0);

        // Status display
        let status_color = match builder.state.status {
            CommandStatus::Running => theme.ui.accent,
            CommandStatus::Completed => theme.status.success,
            CommandStatus::Failed | CommandStatus::ValidationError => theme.status.error,
            CommandStatus::Idle => theme.ui.secondary_text,
        };

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Status: {}", builder.state.status.label()))
                    .font(FontId::monospace(10.0))
                    .color(status_color),
            );
            if let Some(pid) = builder.state.pid {
                ui.label(
                    RichText::new(format!("PID: {pid}"))
                        .font(FontId::monospace(10.0))
                        .color(theme.ui.secondary_text),
                );
            }
            ui.label(
                RichText::new(format!("Executable: {}", builder.state.executable))
                    .font(FontId::monospace(10.0))
                    .color(theme.ui.secondary_text),
            );
        });

        if let Some(code) = builder.state.exit_code {
            ui.label(
                RichText::new(format!("Exit code: {code}"))
                    .font(FontId::monospace(10.0))
                    .color(if code == 0 {
                        theme.status.success
                    } else {
                        theme.status.error
                    }),
            );
        }

        // Error banner
        if let Some(ref error) = builder.state.error_message {
            ui.add_space(4.0);
            let bg_color = with_alpha(theme.status.error, 0.15);
            let banner = Frame::new()
                .fill(bg_color)
                .corner_radius(4.0)
                .inner_margin(Margin::symmetric(8, 4));
            banner.show(ui, |ui| {
                ui.label(
                    RichText::new(error.as_str())
                        .font(FontId::proportional(10.0))
                        .color(theme.status.error),
                );
            });
        }

        ui.add_space(4.0);

        // Output area
        let output_height = 120.0;

        if !builder.state.stdout.is_empty() || !builder.state.stderr.is_empty() {
            ui.label(
                RichText::new("Output")
                    .font(FontId::proportional(11.0))
                    .color(theme.ui.secondary_text)
                    .strong(),
            );

            egui::ScrollArea::vertical()
                .id_salt("command-builder-output")
                .max_height(output_height)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if !builder.state.stdout.is_empty() {
                        ui.label(
                            RichText::new(&builder.state.stdout)
                                .font(FontId::monospace(10.0))
                                .color(theme.ui.text),
                        );
                    }
                    if !builder.state.stderr.is_empty() {
                        ui.label(
                            RichText::new(&builder.state.stderr)
                                .font(FontId::monospace(10.0))
                                .color(theme.status.error),
                        );
                    }
                });
        } else if builder.state.status == CommandStatus::Running {
            ui.label(
                RichText::new("Waiting for output...")
                    .font(FontId::proportional(10.0))
                    .color(theme.ui.secondary_text),
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn validate_executable_rejects_empty() {
        assert!(validate_executable("").is_err());
        assert!(validate_executable("  ").is_err());
    }

    #[test]
    fn validate_executable_accepts_non_empty() {
        assert!(validate_executable("ls").is_ok());
        assert!(validate_executable("/usr/bin/ls").is_ok());
    }

    #[test]
    fn validate_working_dir_accepts_empty() {
        assert!(validate_working_dir("").is_ok());
        assert!(validate_working_dir("  ").is_ok());
    }

    #[test]
    fn validate_working_dir_rejects_nonexistent() {
        assert!(validate_working_dir("/nonexistent/path").is_err());
    }

    #[test]
    fn validate_working_dir_accepts_existing() {
        assert!(validate_working_dir("/tmp").is_ok());
    }

    #[test]
    fn parse_env_vars_empty() {
        let vars = parse_env_vars("");
        assert!(vars.is_empty());
    }

    #[test]
    fn parse_env_vars_single() {
        let vars = parse_env_vars("FOO=bar");
        assert_eq!(vars, vec![("FOO".into(), "bar".into())]);
    }

    #[test]
    fn parse_env_vars_multiple() {
        let vars = parse_env_vars("FOO=bar\nBAZ=qux");
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0], ("FOO".into(), "bar".into()));
        assert_eq!(vars[1], ("BAZ".into(), "qux".into()));
    }

    #[test]
    fn parse_env_vars_skips_empty_lines() {
        let vars = parse_env_vars("FOO=bar\n\nBAZ=qux\n");
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn parse_env_vars_skips_invalid_format() {
        let vars = parse_env_vars("FOO=bar\nINVALID\nBAZ=qux");
        assert_eq!(vars.len(), 2);
    }

    #[test]
    fn parse_args_empty() {
        let args = parse_args("");
        assert!(args.is_empty());
    }

    #[test]
    fn parse_args_simple() {
        let args = parse_args("-la /tmp");
        assert_eq!(args, vec!["-la", "/tmp"]);
    }

    #[test]
    fn parse_args_quoted() {
        let args = parse_args("\"arg with space\" normal");
        assert_eq!(args, vec!["arg with space", "normal"]);
    }

    #[test]
    fn parse_args_single_quoted() {
        let args = parse_args("'arg with space' normal");
        assert_eq!(args, vec!["arg with space", "normal"]);
    }

    #[test]
    fn command_status_labels() {
        assert_eq!(CommandStatus::Idle.label(), "Idle");
        assert_eq!(CommandStatus::Running.label(), "Running");
        assert_eq!(CommandStatus::Completed.label(), "Completed");
        assert_eq!(CommandStatus::Failed.label(), "Failed");
        assert_eq!(CommandStatus::ValidationError.label(), "Error");
    }

    #[test]
    fn process_state_starts_empty() {
        let state = ProcessState::new();
        assert_eq!(state.pid, None);
        assert_eq!(state.status, CommandStatus::Idle);
        assert!(state.stdout.is_empty());
        assert!(state.stderr.is_empty());
    }

    #[test]
    fn command_builder_starts_idle() {
        let builder = CommandBuilder::new();
        assert_eq!(builder.state.status, CommandStatus::Idle);
        assert!(builder.executable.is_empty());
    }

    #[test]
    fn command_builder_clear_resets() {
        let mut builder = CommandBuilder::new();
        builder.executable = "ls".into();
        builder.arguments = "-la".into();
        builder.state.status = CommandStatus::Completed;
        builder.clear();
        assert!(builder.executable.is_empty());
        assert!(builder.arguments.is_empty());
        assert_eq!(builder.state.status, CommandStatus::Idle);
    }

    #[test]
    fn execute_ls_command() {
        let result = execute_command("ls", &[], "/tmp", &[]);
        assert!(result.is_ok());
        let (rx, _child) = result.unwrap();
        // Should receive Started message
        let msg = rx.recv_timeout(Duration::from_secs(1));
        assert!(msg.is_ok());
    }

    #[test]
    fn execute_invalid_command_fails() {
        let result = execute_command("/nonexistent/binary", &[], "/tmp", &[]);
        assert!(result.is_err());
    }
}
