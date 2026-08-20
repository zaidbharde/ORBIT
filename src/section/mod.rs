pub mod placeholder;
pub mod registry;
pub mod system;
pub mod terminal_section;

use crate::config::{AppearanceConfig, GlassConfig, TypographyConfig};
use crate::theme::Theme;
use eframe::egui;

/// Unique identifier for a built-in ORBIT section.
///
/// Sections are internal modules of the same ORBIT application — they are
/// never separate processes or workspaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionId {
    Terminal,
    Coding,
    Networking,
    Cybersecurity,
    DevOps,
    System,
}

/// Static metadata describing a section. Kept data-driven so the UI
/// (navigation rail, command palette) never hard-codes section names.
pub struct SectionDescriptor {
    pub name: &'static str,
    /// Short glyph shown in the navigation rail.
    pub icon: &'static str,
    pub description: &'static str,
    /// Keyboard shortcut label, e.g. "Ctrl+1".
    pub shortcut: &'static str,
}

pub const SECTION_DESCRIPTORS: [SectionDescriptor; 6] = [
    SectionDescriptor {
        name: "Terminal",
        icon: ">_",
        description: "The ORBIT terminal: PTY shell with tabs, panes, splits, search and history.",
        shortcut: "Ctrl+1",
    },
    SectionDescriptor {
        name: "Coding",
        icon: "</>",
        description: "Development tools for writing, building and testing software.",
        shortcut: "Ctrl+2",
    },
    SectionDescriptor {
        name: "Networking",
        icon: "<->",
        description: "Network diagnostics, interfaces and connectivity tools.",
        shortcut: "Ctrl+3",
    },
    SectionDescriptor {
        name: "Cybersecurity",
        icon: "#",
        description: "Security utilities, scanning and diagnostics.",
        shortcut: "Ctrl+4",
    },
    SectionDescriptor {
        name: "DevOps",
        icon: "{}",
        description: "Containers, infrastructure and workflow automation.",
        shortcut: "Ctrl+5",
    },
    SectionDescriptor {
        name: "System",
        icon: "[]",
        description: "System monitoring, processes and services.",
        shortcut: "Ctrl+6",
    },
];

impl SectionId {
    /// Built-in sections in their canonical (default) order.
    pub const ALL: [SectionId; 6] = [
        SectionId::Terminal,
        SectionId::Coding,
        SectionId::Networking,
        SectionId::Cybersecurity,
        SectionId::DevOps,
        SectionId::System,
    ];

    /// Stable index within [`SectionId::ALL`]. Terminal is always 0.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn descriptor(self) -> &'static SectionDescriptor {
        &SECTION_DESCRIPTORS[self.index()]
    }

    pub fn name(self) -> &'static str {
        self.descriptor().name
    }

    /// Stable config id ("terminal", "coding", ...). Persisted in the config
    /// file and restored on startup.
    pub fn config_id(self) -> &'static str {
        match self {
            SectionId::Terminal => "terminal",
            SectionId::Coding => "coding",
            SectionId::Networking => "networking",
            SectionId::Cybersecurity => "cybersecurity",
            SectionId::DevOps => "devops",
            SectionId::System => "system",
        }
    }

    /// Parses a persisted config id. Invalid or unknown ids safely fall back
    /// to the Terminal section, which is always enabled and always valid.
    pub fn from_config_id(id: &str) -> SectionId {
        SectionId::ALL
            .iter()
            .copied()
            .find(|section| section.config_id() == id)
            .unwrap_or(SectionId::Terminal)
    }
}

/// Actions a section can perform. Sections that do not support an action
/// simply ignore it (default trait implementation), so the global command
/// palette and keyboard shortcuts stay valid in every section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionAction {
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    SplitHorizontal,
    SplitVertical,
    ClosePane,
    RestartPane,
    ToggleSearch,
    ToggleHistory,
    CopySelection,
    FindNext,
    FindPrevious,
    NextPane,
    PreviousPane,
    PageUp,
    PageDown,
}

/// Everything a section needs from the global ORBIT chrome to render
/// section-specific UI. Global settings (theme, typography, glass,
/// appearance) are read-only here — they live in the app and are never
/// owned by a section.
pub struct SectionContext<'a> {
    pub theme: &'a Theme,
    pub typography: &'a TypographyConfig,
    pub glass: &'a GlassConfig,
    pub appearance: &'a AppearanceConfig,
    /// Glass-aware panel/chrome fill color, precomputed by the app.
    pub panel_fill: egui::Color32,
}

/// A built-in ORBIT section.
///
/// Sections are constructed once at startup and live for the whole session,
/// so their state (e.g. terminal tabs and panes) survives section switches.
/// Only the active section is updated and rendered each frame.
pub trait Section {
    fn id(&self) -> SectionId;

    /// Advance section-internal state (e.g. draining PTY output). Called
    /// once per frame for the active section only.
    fn update(&mut self, _ctx: &egui::Context) {}

    /// Render the section's main content in the central panel. Returns the
    /// focusable surface response (or a dummy response for sections without
    /// focusable content).
    fn render(&mut self, ui: &mut egui::Ui, context: &SectionContext<'_>) -> egui::Response;

    /// Section-specific chrome rendered inside the top bar below the global
    /// controls (e.g. terminal tabs and the search bar).
    fn top_bar(&mut self, _ui: &mut egui::Ui, _context: &SectionContext<'_>) {}

    /// Section-specific overlays such as side panels. Called for the active
    /// section only.
    fn overlays(&mut self, _ctx: &egui::Context, _context: &SectionContext<'_>) {}

    /// Keyboard input while this section is active. The app consumes global
    /// shortcuts first and only forwards the remaining events.
    fn handle_keyboard(&mut self, _ctx: &egui::Context, _event: &egui::Event, _focused: bool) {}

    /// Execute a global action this section supports (no-op otherwise).
    fn action(&mut self, _action: SectionAction, _ctx: &egui::Context) {}

    /// Optional status line shown in the global chrome, e.g. "running" for
    /// the terminal shell.
    fn status_label(&self, _theme: &Theme) -> Option<(String, egui::Color32)> {
        None
    }

    /// Called right after this section becomes active, so it can restore
    /// keyboard focus to its content (e.g. the terminal pane).
    fn on_activated(&mut self, _ctx: &egui::Context) {}
}
