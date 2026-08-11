use crate::terminal::TerminalGrid;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct TerminalConfig {
    pub shell: String,
    pub working_dir: PathBuf,
    pub initial_grid: TerminalGrid,
    pub scrollback_lines: usize,
    pub theme: String,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: default_shell(),
            working_dir: default_working_dir(),
            initial_grid: TerminalGrid { rows: 24, cols: 80 },
            scrollback_lines: 10_000,
            theme: "orbit-dark".to_owned(),
        }
    }
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned())
}

fn default_working_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}
