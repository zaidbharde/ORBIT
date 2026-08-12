use crate::terminal::TerminalGrid;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TypographyConfig {
    pub terminal_font: String,
    pub terminal_font_size: f32,
    pub line_spacing: f32,
    pub character_spacing: f32,
    pub ui_font_size: f32,
}

impl Default for TypographyConfig {
    fn default() -> Self {
        Self {
            terminal_font: default_system_monospace_font(),
            terminal_font_size: 14.0,
            line_spacing: 0.0,
            character_spacing: 0.0,
            ui_font_size: 14.0,
        }
    }
}

impl TypographyConfig {
    pub fn available_font_names() -> Vec<String> {
        let mut names = TypographyConfig::system_font_candidates();
        let mut seen = std::collections::BTreeSet::new();
        names.retain(|name| seen.insert(name.clone()));
        names
    }

    pub fn system_font_candidates() -> Vec<String> {
        let mut values = vec![
            "JetBrains Mono".to_owned(),
            "Fira Code".to_owned(),
            "Cascadia Code".to_owned(),
            "Source Code Pro".to_owned(),
            "DejaVu Sans Mono".to_owned(),
            "Noto Sans Mono".to_owned(),
            "Liberation Mono".to_owned(),
            "Monospace".to_owned(),
        ];
        let mut seen = std::collections::BTreeSet::new();
        values.retain(|value| seen.insert(value.clone()));
        values
    }

    pub fn resolved_terminal_font_name(&self) -> String {
        let trimmed = self.terminal_font.trim();
        if !trimmed.is_empty() && self.font_file_for(trimmed).is_some() {
            return trimmed.to_owned();
        }

        let fallback = default_system_monospace_font();
        if self.font_file_for(&fallback).is_some() {
            fallback
        } else {
            "monospace".to_owned()
        }
    }

    pub fn terminal_font_family(&self) -> egui::FontFamily {
        let font_name = self.resolved_terminal_font_name();
        if font_name == "monospace" {
            egui::FontFamily::Monospace
        } else {
            egui::FontFamily::Name(font_name.into())
        }
    }

    pub fn terminal_font_id(&self) -> egui::FontId {
        egui::FontId::new(self.terminal_font_size, self.terminal_font_family())
    }

    pub fn cell_width(&self) -> f32 {
        self.terminal_font_size * 0.6 + self.character_spacing.max(0.0)
    }

    pub fn cell_height(&self) -> f32 {
        self.terminal_font_size + self.line_spacing.max(0.0)
    }

    pub fn install_for_egui(&self, fonts: &mut egui::FontDefinitions) {
        let family = self.resolved_terminal_font_name();
        if family == "monospace" {
            return;
        }

        if let Some(file) = self.font_file_for(&family) {
            let bytes = match fs::read(file) {
                Ok(bytes) => bytes,
                Err(_) => return,
            };
            let key = "orbit-terminal-font";
            fonts.font_data.insert(
                key.to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            let monospace = fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default();
            if !monospace.iter().any(|name| name == key) {
                monospace.insert(0, key.to_owned());
            }
        }
    }

    fn font_file_for(&self, family: &str) -> Option<PathBuf> {
        let trimmed = family.trim();
        if trimmed.is_empty() {
            return None;
        }

        let expected = family_name_variants(trimmed);
        for candidate in expected {
            if let Ok(output) = std::process::Command::new("fc-match")
                .args(["--format=%{file}\n", &candidate])
                .output()
            {
                if output.status.success() {
                    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                    if !value.is_empty() {
                        let path = PathBuf::from(value);
                        if path.exists() {
                            return Some(path);
                        }
                    }
                }
            }
            let path = find_font_in_standard_directories(&candidate);
            if path.is_some() {
                return path;
            }
        }
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    pub shell: String,
    pub working_dir: PathBuf,
    pub initial_grid: TerminalGrid,
    pub scrollback_lines: usize,
    pub theme: String,
    pub typography: TypographyConfig,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: default_shell(),
            working_dir: default_working_dir(),
            initial_grid: TerminalGrid { rows: 24, cols: 80 },
            scrollback_lines: 10_000,
            theme: "orbit-dark".to_owned(),
            typography: TypographyConfig::default(),
        }
    }
}

impl TerminalConfig {
    pub fn load() -> Self {
        let path = config_path();
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => return Self::default(),
        };

        match toml::from_str::<Self>(&content) {
            Ok(mut config) => {
                if config.typography.terminal_font.trim().is_empty() {
                    config.typography.terminal_font = default_system_monospace_font();
                }
                config
            }
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let payload = toml::to_string_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
        fs::write(path, payload)
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

fn config_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".orbit_config")
}

fn default_system_monospace_font() -> String {
    for candidate in TypographyConfig::system_font_candidates() {
        if TypographyConfig::default()
            .font_file_for(&candidate)
            .is_some()
        {
            return candidate;
        }
    }
    "monospace".to_owned()
}

fn family_name_variants(family: &str) -> Vec<String> {
    let mut variants = vec![family.to_owned()];
    let lower = family.to_ascii_lowercase();
    if !variants
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&lower))
    {
        variants.push(lower);
    }
    variants
}

fn find_font_in_standard_directories(family: &str) -> Option<PathBuf> {
    let directories = [
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/usr/share/fonts/truetype",
        "/usr/share/fonts/opentype",
        "/usr/local/share/fonts/truetype",
        "/usr/local/share/fonts/opentype",
    ];

    let lower = family.to_ascii_lowercase();
    for dir in directories {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let full = path.to_string_lossy().to_ascii_lowercase();
            if full.contains(&lower) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{TerminalConfig, TypographyConfig};

    #[test]
    fn typography_defaults_are_sane() {
        let config = TerminalConfig::default();
        assert!(config.typography.terminal_font_size > 0.0);
        assert!(config.typography.ui_font_size > 0.0);
        assert!(config.typography.line_spacing >= 0.0);
        assert!(config.typography.character_spacing >= 0.0);
    }

    #[test]
    fn invalid_font_falls_back_to_default_monospace() {
        let mut config = TypographyConfig::default();
        config.terminal_font = "Definitely Not A Real Font Name 123".to_string();
        let fallback = config.resolved_terminal_font_name();
        assert!(!fallback.trim().is_empty());
        assert_ne!(fallback, "Definitely Not A Real Font Name 123");
    }
}
