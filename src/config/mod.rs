use crate::terminal::TerminalGrid;
use crate::workspace::Workspace;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
            terminal_font: "monospace".to_owned(),
            terminal_font_size: 14.0,
            line_spacing: 0.0,
            character_spacing: 0.0,
            ui_font_size: 14.0,
        }
    }
}

/// Tint presets for the glass material layer.
///
/// The tint only affects the background/material layer; it never recolors
/// terminal text, which is always painted with the theme's own colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlassTint {
    Neutral,
    Purple,
    Pink,
    Cyan,
    Blue,
    Green,
    Custom,
}

impl GlassTint {
    pub const ALL: [GlassTint; 7] = [
        GlassTint::Neutral,
        GlassTint::Purple,
        GlassTint::Pink,
        GlassTint::Cyan,
        GlassTint::Blue,
        GlassTint::Green,
        GlassTint::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GlassTint::Neutral => "Neutral",
            GlassTint::Purple => "Purple",
            GlassTint::Pink => "Pink",
            GlassTint::Cyan => "Cyan",
            GlassTint::Blue => "Blue",
            GlassTint::Green => "Green",
            GlassTint::Custom => "Custom",
        }
    }
}

/// Glassmorphism / acrylic material configuration.
///
/// Kept logically separate from the theme: changing the theme never resets
/// glass settings and changing glass settings never resets the theme.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GlassConfig {
    /// Master switch. When disabled ORBIT renders as a normal opaque terminal.
    pub enabled: bool,
    /// Overall opacity of the glass layer (1.0 = fully opaque).
    pub opacity: f32,
    /// Tint applied to the glass layer only.
    pub tint: GlassTint,
    /// How strongly the tint color is mixed into the theme background.
    pub tint_opacity: f32,
    /// Blur strength. Only meaningful on backends that expose native blur
    /// (e.g. KWin via the X11 blur region). On other compositors it is stored
    /// for future use and has no visual effect.
    pub blur_strength: f32,
    /// RGB values used when `tint == GlassTint::Custom`.
    pub custom_tint: [u8; 3],
}

impl Default for GlassConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            opacity: 0.82,
            tint: GlassTint::Neutral,
            tint_opacity: 0.30,
            blur_strength: 4.0,
            custom_tint: [110, 70, 180],
        }
    }
}

/// Cursor shape for the terminal insertion point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    Block,
    Beam,
    Underline,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self::Block
    }
}

impl CursorStyle {
    pub const ALL: [CursorStyle; 3] = [
        CursorStyle::Block,
        CursorStyle::Beam,
        CursorStyle::Underline,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CursorStyle::Block => "Block",
            CursorStyle::Beam => "Beam",
            CursorStyle::Underline => "Underline",
        }
    }
}

/// How fast the cursor blinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorBlinkSpeed {
    Slow,
    Normal,
    Fast,
}

impl Default for CursorBlinkSpeed {
    fn default() -> Self {
        Self::Normal
    }
}

impl CursorBlinkSpeed {
    pub const ALL: [CursorBlinkSpeed; 3] = [
        CursorBlinkSpeed::Slow,
        CursorBlinkSpeed::Normal,
        CursorBlinkSpeed::Fast,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CursorBlinkSpeed::Slow => "Slow",
            CursorBlinkSpeed::Normal => "Normal",
            CursorBlinkSpeed::Fast => "Fast",
        }
    }

    /// Full blink period in seconds (visible phase + hidden phase).
    pub fn period(self) -> f32 {
        match self {
            CursorBlinkSpeed::Slow => 1.2,
            CursorBlinkSpeed::Normal => 0.7,
            CursorBlinkSpeed::Fast => 0.4,
        }
    }

    /// Whether the cursor is in its visible phase at `time` (seconds, e.g.
    /// `egui::Context::input().time`). Pure function of time: no timers or
    /// animation loops are involved.
    pub fn on_at(self, time: f64) -> bool {
        (time / self.period() as f64).fract() < 0.5
    }
}

/// Where the cursor color comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorColorMode {
    Theme,
    Custom,
}

impl Default for CursorColorMode {
    fn default() -> Self {
        Self::Theme
    }
}

impl CursorColorMode {
    pub const ALL: [CursorColorMode; 2] = [CursorColorMode::Theme, CursorColorMode::Custom];

    pub fn label(self) -> &'static str {
        match self {
            CursorColorMode::Theme => "Theme",
            CursorColorMode::Custom => "Custom",
        }
    }
}

/// Cursor & general appearance settings.
///
/// Kept logically separate from typography, theme and glass: changing any of
/// those never resets appearance settings and vice versa.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppearanceConfig {
    /// Shape of the terminal cursor.
    pub cursor_style: CursorStyle,
    /// Whether the cursor blinks when the terminal is focused.
    pub cursor_blink: bool,
    /// Blink speed (period) when `cursor_blink` is on.
    pub cursor_blink_speed: CursorBlinkSpeed,
    /// Where the cursor color comes from.
    pub cursor_color_mode: CursorColorMode,
    /// RGB used when `cursor_color_mode == CursorColorMode::Custom`.
    pub cursor_custom_color: [u8; 3],
    /// Thickness in points for Beam/Underline cursors.
    pub cursor_thickness: f32,
    /// Corner radius in points for panels and the terminal surface.
    pub panel_radius: f32,
    /// Border width in points for the terminal surface and panels.
    pub border_width: f32,
    /// Border opacity (0.0 = invisible, 1.0 = opaque).
    pub border_opacity: f32,
    /// Multiplier for UI spacing (toolbar, tabs, panels).
    pub spacing_scale: f32,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            cursor_style: CursorStyle::Block,
            cursor_blink: true,
            cursor_blink_speed: CursorBlinkSpeed::Normal,
            cursor_color_mode: CursorColorMode::Theme,
            cursor_custom_color: [120, 190, 255],
            cursor_thickness: 2.0,
            panel_radius: 10.0,
            border_width: 1.0,
            border_opacity: 0.55,
            spacing_scale: 1.0,
        }
    }
}

const PREFERRED_FONT_CANDIDATES: &[&str] = &[
    "JetBrains Mono",
    "Fira Code",
    "Cascadia Code",
    "Source Code Pro",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "Noto Mono",
    "Ubuntu Mono",
    "Ubuntu Sans Mono",
    "Nimbus Mono PS",
];

/// Fonts fontconfig classifies as monospace but which are not usable as
/// terminal fonts (e.g. emoji fonts where ASCII digit widths differ, or
/// sign-language fonts without a Latin alphabet).
const NON_TERMINAL_FONT_MARKERS: &[&str] = &["emoji", "signwriting"];

const ASCII_MONOSPACE_PROBE: &str =
    "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

impl TypographyConfig {
    /// Fonts that are actually installed on the system, genuinely monospace,
    /// and safe to bind into egui. The list is discovered once per process.
    pub fn available_font_names() -> Vec<String> {
        AVAILABLE_FONTS
            .get_or_init(|| {
                let mut names = Vec::new();
                let mut seen = std::collections::BTreeSet::new();
                for name in TypographyConfig::system_font_candidates()
                    .into_iter()
                    .chain(discovered_monospace_families())
                {
                    let trimmed = name.trim();
                    if trimmed.is_empty() || !seen.insert(trimmed.to_owned()) {
                        continue;
                    }
                    if is_usable_terminal_font(trimmed) {
                        names.push(trimmed.to_owned());
                    }
                }
                names
            })
            .clone()
    }

    pub fn system_font_candidates() -> Vec<String> {
        let mut values = PREFERRED_FONT_CANDIDATES
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        values.push("Monospace".to_owned());
        let mut seen = std::collections::BTreeSet::new();
        values.retain(|value| seen.insert(value.clone()));
        values
    }

    /// The font family that will actually be rendered in the terminal.
    ///
    /// Returns the user's selection only when it maps to an installed,
    /// monospace font file. Otherwise it falls back to the first usable
    /// system monospace font, never a non-existent or proportional font.
    pub fn resolved_terminal_font_name(&self) -> String {
        let trimmed = self.terminal_font.trim();
        if !trimmed.is_empty() && usable_terminal_font_file(trimmed).is_some() {
            return trimmed.to_owned();
        }

        let fallback = default_system_monospace_font();
        if usable_terminal_font_file(&fallback).is_some() {
            fallback
        } else {
            "monospace".to_owned()
        }
    }

    pub fn terminal_font_id(&self) -> egui::FontId {
        egui::FontId::new(self.terminal_font_size, egui::FontFamily::Monospace)
    }

    /// Width of one terminal cell in points.
    ///
    /// Uses the resolved font's real monospace advance width so columns always
    /// match the rendered glyph width, no matter which font is installed.
    pub fn cell_width(&self) -> f32 {
        let size = self.terminal_font_size.max(1.0);
        let base = terminal_font_metrics(&self.resolved_terminal_font_name())
            .map(|m| m.advance_per_pt * size)
            .unwrap_or(size * 0.6);
        (base + self.character_spacing.max(0.0)).max(1.0)
    }

    /// Height of one terminal cell in points.
    ///
    /// Uses the resolved font's natural line height so rows never overlap,
    /// plus the configured extra line spacing.
    pub fn cell_height(&self) -> f32 {
        let size = self.terminal_font_size.max(1.0);
        let base = terminal_font_metrics(&self.resolved_terminal_font_name())
            .map(|m| m.height_per_pt * size)
            .unwrap_or(size);
        (base + self.line_spacing.max(0.0)).max(1.0)
    }

    /// Loads the resolved terminal font into `egui::FontDefinitions` and binds
    /// it as the first choice for `FontFamily::Monospace`, so the terminal
    /// renderer (which paints with `FontId` of `FontFamily::Monospace`) uses
    /// the selected font for every glyph.
    pub fn install_for_egui(&self, fonts: &mut egui::FontDefinitions) {
        let family = self.resolved_terminal_font_name();
        if family == "monospace" {
            return;
        }

        let Some(file) = usable_terminal_font_file(&family) else {
            return;
        };
        let Ok(bytes) = fs::read(file) else {
            return;
        };

        let key = format!("orbit-terminal-font-{}", family.replace(' ', "_"));
        fonts.font_data.insert(
            key.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );

        let custom_family = egui::FontFamily::Name(family.clone().into());
        let custom = fonts.families.entry(custom_family).or_default();
        if !custom.iter().any(|name| name == &key) {
            custom.insert(0, key.clone());
        }

        let monospace = fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default();
        if !monospace.iter().any(|name| name == &key) {
            monospace.insert(0, key);
        }
    }
}

static FONT_FILE_CACHE: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();

static USABLE_FONT_CACHE: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();

static FONT_METRICS_CACHE: OnceLock<Mutex<HashMap<String, Option<FontMetrics>>>> = OnceLock::new();

static AVAILABLE_FONTS: OnceLock<Vec<String>> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
struct FontMetrics {
    /// Monospace advance width in points for a terminal font size of 1.0.
    advance_per_pt: f32,
    /// Natural line box height in points for a terminal font size of 1.0.
    height_per_pt: f32,
}

/// Resolves `family` to an installed font file that is genuinely monospace
/// (all ASCII digits and letters share the same advance width) and not a
/// known non-terminal font. The result is cached.
fn usable_terminal_font_file(family: &str) -> Option<PathBuf> {
    let trimmed = family.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if NON_TERMINAL_FONT_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return None;
    }

    let cache = USABLE_FONT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache
        .lock()
        .ok()
        .and_then(|guard| guard.get(trimmed).cloned())
    {
        return hit;
    }

    let result = font_file_for_family(trimmed).filter(|file| font_is_monospace(file));
    if let Ok(mut guard) = cache.lock() {
        guard.insert(trimmed.to_owned(), result.clone());
    }
    result
}

/// True when `family` resolves to an installed font file that is genuinely
/// monospace (all ASCII digits and letters share the same advance width).
fn is_usable_terminal_font(family: &str) -> bool {
    usable_terminal_font_file(family).is_some()
}

/// Cached advance/line-height metrics for a usable terminal font, measured in
/// the same way egui lays out glyphs (so cell geometry matches rendering).
fn terminal_font_metrics(family: &str) -> Option<FontMetrics> {
    let trimmed = family.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cache = FONT_METRICS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache
        .lock()
        .ok()
        .and_then(|guard| guard.get(trimmed).copied())
    {
        return hit;
    }

    let result = measure_font_metrics(trimmed);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(trimmed.to_owned(), result);
    }
    result
}

fn measure_font_metrics(family: &str) -> Option<FontMetrics> {
    use ab_glyph::{Font as _, PxScale, ScaleFont as _};

    let file = usable_terminal_font_file(family)?;
    let bytes = fs::read(file).ok()?;
    let font = ab_glyph::FontVec::try_from_vec(bytes).ok()?;
    let units_per_em = font.units_per_em()?;
    if !(16.0..=16_384.0).contains(&units_per_em) {
        return None;
    }

    // egui scales a font so that `size * height_unscaled / units_per_em`
    // becomes the pixel-per-em. Measure at a font size of 1.0 point and let
    // callers scale linearly with the actual size.
    let pixels_per_em = PxScale::from(1.0 * font.height_unscaled() / units_per_em);
    let scaled = font.as_scaled(pixels_per_em);
    let advance = scaled.h_advance(font.glyph_id('M'));
    let height = scaled.ascent() - scaled.descent() + scaled.line_gap();
    if advance <= 0.0 || height <= 0.0 {
        return None;
    }

    Some(FontMetrics {
        advance_per_pt: advance,
        height_per_pt: height,
    })
}

fn discovered_monospace_families() -> Vec<String> {
    let mut families = Vec::new();
    let Ok(output) = std::process::Command::new("fc-list")
        .args([":spacing=100", "--format=%{family[0]}\n"])
        .output()
    else {
        return families;
    };
    if !output.status.success() {
        return families;
    }

    let mut seen = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let name = line.trim();
        if name.is_empty() || !seen.insert(name.to_owned()) {
            continue;
        }
        families.push(name.to_owned());
    }
    families
}

fn font_is_monospace(path: &Path) -> bool {
    use ab_glyph::{Font as _, ScaleFont as _};

    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let Ok(font) = ab_glyph::FontVec::try_from_vec(bytes) else {
        return false;
    };
    // egui panics when a font's units-per-em is outside this range; reject
    // such fonts up front so a runtime font switch can never crash.
    let units_per_em = font.units_per_em().unwrap_or(0.0);
    if !(16.0..=16_384.0).contains(&units_per_em) {
        return false;
    }
    let Some(scale) = font.pt_to_px_scale(12.0) else {
        return false;
    };
    let scaled = font.as_scaled(scale);

    let mut reference: Option<f32> = None;
    for c in ASCII_MONOSPACE_PROBE.chars() {
        let advance = scaled.h_advance(font.glyph_id(c));
        if advance <= 0.0 {
            return false;
        }
        match reference {
            None => reference = Some(advance),
            Some(expected) => {
                if (advance - expected).abs() > 0.05 {
                    return false;
                }
            }
        }
    }
    reference.is_some()
}

fn font_file_for_family(family: &str) -> Option<PathBuf> {
    let trimmed = family.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cache = FONT_FILE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache
        .lock()
        .ok()
        .and_then(|guard| guard.get(trimmed).cloned())
    {
        return hit;
    }

    let result = font_file_for_family_uncached(trimmed);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(trimmed.to_owned(), result.clone());
    }
    result
}

fn font_file_for_family_uncached(family: &str) -> Option<PathBuf> {
    let expected = family_name_variants(family);
    for candidate in expected {
        if let Ok(output) = std::process::Command::new("fc-match")
            .args(["--format=%{family[0]}|%{file}\n", &candidate])
            .output()
        {
            if output.status.success() {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if let Some((matched_family, matched_file)) = value.split_once('|') {
                    if matched_family.eq_ignore_ascii_case(family) {
                        let path = PathBuf::from(matched_file.trim());
                        if path.exists() {
                            return Some(path);
                        }
                    }
                }
            }
        }
        if let Some(path) = find_font_in_standard_directories(&candidate) {
            return Some(path);
        }
    }
    None
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
    pub glass: GlassConfig,
    pub appearance: AppearanceConfig,
    /// Saved workspaces (metadata plus serialized layouts). Order is the
    /// user-visible workspace order.
    pub workspaces: Vec<Workspace>,
    /// Id of the workspace that was active on the last run.
    pub active_workspace: String,
    /// Id of the workspace marked as default.
    pub default_workspace: String,
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
            glass: GlassConfig::default(),
            appearance: AppearanceConfig::default(),
            workspaces: Vec::new(),
            active_workspace: String::new(),
            default_workspace: String::new(),
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
            Err(err) => {
                if std::env::var_os("ORBIT_DEBUG_EVENTS").is_some() {
                    eprintln!("[DBG] config parse error: {err}");
                }
                Self::default()
            }
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
        if is_usable_terminal_font(&candidate) {
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
    use super::{
        AppearanceConfig, CursorBlinkSpeed, CursorColorMode, CursorStyle, TerminalConfig,
        TypographyConfig, default_system_monospace_font, font_file_for_family, font_is_monospace,
    };
    use eframe::egui;

    #[test]
    fn appearance_defaults_are_sane() {
        let appearance = AppearanceConfig::default();
        assert_eq!(appearance.cursor_style, CursorStyle::Block);
        assert!(appearance.cursor_blink);
        assert_eq!(appearance.cursor_blink_speed, CursorBlinkSpeed::Normal);
        assert_eq!(appearance.cursor_color_mode, CursorColorMode::Theme);
        assert!(appearance.cursor_thickness > 0.0);
        assert!((0.0..=16.0).contains(&appearance.panel_radius));
        assert!((0.0..=4.0).contains(&appearance.border_width));
        assert!((0.0..=1.0).contains(&appearance.border_opacity));
        assert!((0.8..=1.4).contains(&appearance.spacing_scale));
    }

    #[test]
    fn appearance_round_trips_through_toml() {
        let config = TerminalConfig::default();
        let payload = toml::to_string(&config).unwrap();
        let loaded: TerminalConfig = toml::from_str(&payload).unwrap();
        assert_eq!(config.appearance, loaded.appearance);
    }

    #[test]
    fn missing_appearance_section_falls_back_to_defaults() {
        let payload = "shell = \"/bin/bash\"\ntheme = \"frost\"\n";
        let loaded: TerminalConfig = toml::from_str(payload).unwrap();
        assert_eq!(loaded.appearance, AppearanceConfig::default());
        assert_eq!(loaded.theme, "frost");
    }

    #[test]
    fn blink_speed_periods_are_positive_and_ordered() {
        let slow = CursorBlinkSpeed::Slow.period();
        let normal = CursorBlinkSpeed::Normal.period();
        let fast = CursorBlinkSpeed::Fast.period();
        assert!(slow > normal && normal > fast && fast > 0.0);
    }

    #[test]
    fn blink_phase_is_a_pure_function_of_time() {
        // The visible phase must repeat with the configured period.
        let speed = CursorBlinkSpeed::Normal;
        let period = speed.period() as f64;
        assert!(speed.on_at(0.1));
        assert!(speed.on_at(0.1 + period));
        assert_eq!(speed.on_at(0.1), speed.on_at(0.1 + 2.0 * period));
        // A point in the second half of the period is hidden.
        assert!(!speed.on_at(0.6 * period));
    }

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

    #[test]
    fn every_offered_font_is_installed_and_monospace() {
        let names = TypographyConfig::available_font_names();
        assert!(
            !names.is_empty(),
            "at least one system monospace font exists"
        );
        for name in names {
            let file = font_file_for_family(&name)
                .unwrap_or_else(|| panic!("font {name:?} has no font file"));
            assert!(
                font_is_monospace(&file),
                "offered font {name:?} at {file:?} is not genuinely monospace"
            );
        }
    }

    #[test]
    fn fonts_that_are_not_installed_are_not_offered() {
        let names = TypographyConfig::available_font_names();
        for fake in [
            "JetBrains Mono",
            "Fira Code",
            "Cascadia Code",
            "Source Code Pro",
        ] {
            if font_file_for_family(fake).is_none() {
                assert!(
                    !names.iter().any(|name| name == fake),
                    "uninstalled font {fake:?} must not be offered"
                );
            }
        }
    }

    #[test]
    fn proportional_system_font_is_never_used_as_terminal_font() {
        // A known proportional font (if installed) must never become the
        // resolved terminal font, even if explicitly requested.
        let mut config = TypographyConfig::default();
        config.terminal_font = "Noto Sans".to_owned();
        let resolved = config.resolved_terminal_font_name();
        if font_file_for_family("Noto Sans").is_some() {
            assert!(
                !resolved.eq_ignore_ascii_case("Noto Sans"),
                "proportional font must not be used as the terminal font"
            );
        }
        assert_ne!(resolved, "monospace");
    }

    #[test]
    fn selected_installed_font_is_resolved_and_bound() {
        let family = default_system_monospace_font();
        if family == "monospace" {
            return;
        }

        let config = TypographyConfig {
            terminal_font: family.clone(),
            terminal_font_size: 14.0,
            line_spacing: 0.0,
            character_spacing: 0.0,
            ui_font_size: 14.0,
        };

        assert_eq!(config.resolved_terminal_font_name(), family);

        let mut fonts = egui::FontDefinitions::default();
        config.install_for_egui(&mut fonts);

        assert!(
            fonts
                .families
                .contains_key(&egui::FontFamily::Name(family.clone().into())),
            "custom font family {family:?} should be bound for egui"
        );
        let monospace = fonts.families.get(&egui::FontFamily::Monospace).unwrap();
        assert!(
            monospace
                .first()
                .is_some_and(|name| name.starts_with("orbit-terminal-font-")),
            "the selected font must be first in the Monospace family"
        );
    }

    #[test]
    fn runtime_font_switch_changes_rendered_glyph_metrics() {
        // Mirrors the app's runtime path (`apply_typography`): install the
        // selected font into FontDefinitions, hand them to a live Context via
        // `set_fonts`, run a pass, then measure what Monospace actually
        // renders. Switching fonts must change the measured metrics, proving
        // the selected font reaches the renderer.
        use eframe::egui::Context;
        let names = TypographyConfig::available_font_names();
        if names.len() < 2 {
            return;
        }

        let ctx = Context::default();
        let mut metrics = Vec::new();
        for family in names.iter().take(2) {
            let config = TypographyConfig {
                terminal_font: family.clone(),
                ..Default::default()
            };
            let mut fonts_def = egui::FontDefinitions::default();
            config.install_for_egui(&mut fonts_def);
            ctx.set_fonts(fonts_def);
            ctx.begin_pass(egui::RawInput::default());
            let id = egui::FontId::monospace(config.terminal_font_size);
            metrics.push(ctx.fonts(|f| f.glyph_width(&id, 'M')));
        }
        assert_ne!(
            metrics[0], metrics[1],
            "switching terminal font must change rendered glyph width"
        );
    }

    #[test]
    fn distinct_fonts_have_distinct_glyph_metrics() {
        // The whole point of font switching: two different installed fonts
        // must not render identical glyphs.
        use eframe::egui::epaint::image::AlphaFromCoverage;
        let names = TypographyConfig::available_font_names();
        if names.len() < 2 {
            return;
        }
        let mut widths = Vec::new();
        for family in &names {
            let config = TypographyConfig {
                terminal_font: family.clone(),
                terminal_font_size: 13.0,
                line_spacing: 0.0,
                character_spacing: 0.0,
                ui_font_size: 14.0,
            };
            let mut fonts_def = egui::FontDefinitions::default();
            config.install_for_egui(&mut fonts_def);
            let fonts =
                egui::epaint::text::Fonts::new(1.0, 4096, AlphaFromCoverage::default(), fonts_def);
            let id = egui::FontId::monospace(13.0);
            let mut metrics = Vec::new();
            for c in ['M', 'W', '0', 'i', 'm', 'A'] {
                metrics.push((c, fonts.glyph_width(&id, c)));
            }
            widths.push((family.clone(), metrics));
        }
        let first = &widths[0].1;
        assert!(
            widths.iter().skip(1).any(|(_, m)| m != first),
            "installed fonts must differ in glyph metrics"
        );
    }
}
