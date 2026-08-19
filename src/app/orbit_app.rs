use crate::config::{
    AppearanceConfig, CursorColorMode, CursorStyle, GlassConfig, GlassTint, TerminalConfig,
    TypographyConfig,
};
use crate::glass::GlassBackend;
use crate::glass::backend::{apply_x11_blur_region, x11_window_id};
use crate::pty::{PtyCommand, PtySession};
use crate::terminal::{TerminalGrid, TerminalState};
use crate::workspace::{
    SavedPane, SavedPaneLayout, SavedTab, SplitAxis, Workspace, WorkspaceManager, WorkspacePreset,
    WorkspaceRuntime, new_id, resolve_dir_input, resolve_working_dir,
};
use eframe::egui;
use raw_window_handle::HasWindowHandle;
use std::fs::OpenOptions;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const MAX_SCROLLBACK_OFFSET: usize = 10_000;

pub fn run() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ORBIT")
            .with_app_id("dev.orbit.terminal")
            .with_inner_size([960.0, 640.0])
            // The window is always created with an ARGB-capable surface so the
            // glass material can show the desktop through it at runtime. When
            // glass is disabled every pixel is painted opaque, so the window
            // looks like a normal opaque terminal. Platforms without an alpha
            // visual (X11 without compositor) fall back to an opaque surface
            // automatically and ORBIT keeps working.
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        "ORBIT",
        native_options,
        Box::new(|creation| Ok(Box::new(OrbitApp::new(creation)))),
    )
}

struct OrbitApp {
    config: TerminalConfig,
    manager: WorkspaceManager<TerminalTab>,
    workspace_dialog: Option<WorkspaceDialog>,
    search_open: bool,
    history_open: bool,
    typography_dirty: bool,
    theme_dirty: bool,
    appearance_dirty: bool,
    // Command palette
    command_palette_open: bool,
    palette_filter: String,
    palette_selected: usize,
    palette_just_opened: bool,
    // Theme
    theme_name: String,
    theme: crate::theme::Theme,
    available_themes: Vec<&'static str>,
    // Glass / acrylic material
    glass: GlassConfig,
    glass_settings_open: bool,
    appearance_settings_open: bool,
    backend: &'static GlassBackend,
    last_blur_size: Option<egui::Vec2>,
    debug_pane_id: Option<egui::Id>,
    debug_focus_requested: bool,
    debug_search_focused: bool,
}

enum WorkspaceDialog {
    Create {
        name: String,
        preset: WorkspacePreset,
        dir: String,
    },
    Rename {
        id: String,
        name: String,
    },
}

struct TerminalTab {
    id: usize,
    title: String,
    panes: PaneLayout,
    active_pane: usize,
}

enum PaneLayout {
    Single(TerminalPane),
    Split {
        axis: SplitAxis,
        first: TerminalPane,
        second: TerminalPane,
    },
}

struct TerminalPane {
    id: usize,
    title: String,
    working_dir: PathBuf,
    terminal: TerminalState,
    pty: Result<PtySession, String>,
    last_grid: TerminalGrid,
    scrollback_rows: usize,
    selection: Option<Selection>,
    search_query: String,
    search_current: Option<(usize, usize)>,
    current_command: String,
    history: Vec<CommandHistoryEntry>,
    history_filter: String,
    exited: bool,
}

struct CommandHistoryEntry {
    text: String,
    submitted_at: SystemTime,
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: (usize, usize),
    focus: (usize, usize),
}

#[derive(Clone, Copy, Debug)]
enum AppAction {
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
    Paste,
    FindNext,
    FindPrevious,
    NextPane,
    PreviousPane,
    PageUp,
    PageDown,
    NextWorkspace,
    PreviousWorkspace,
    ToggleCommandPalette,
}

#[derive(Clone, Debug)]
enum PaletteAction {
    App(AppAction),
    SwitchToWorkspace(usize),
    NewWorkspace,
    RenameWorkspace,
    DuplicateWorkspace,
    DeleteWorkspace,
    SetDefaultWorkspace(usize),
    SaveWorkspace,
}

impl OrbitApp {
    fn new(creation: &eframe::CreationContext<'_>) -> Self {
        let mut config = TerminalConfig::load();
        if config.workspaces.is_empty() {
            config.workspaces = crate::workspace::default_workspaces(&config.working_dir);
            let _ = config.save();
        }
        for ws in &mut config.workspaces {
            if ws.preset == WorkspacePreset::Unknown {
                ws.preset = WorkspacePreset::Blank;
            }
        }
        let runtimes = config
            .workspaces
            .iter()
            .map(|ws| Self::build_runtime(&config, ws))
            .collect();
        let manager = WorkspaceManager::new(
            runtimes,
            &config.active_workspace,
            &config.default_workspace,
        );

        let available_themes = crate::theme::get_theme_names();
        // Theme preference: config.theme takes precedence, but allow ~/.orbit_theme override.
        let mut theme_name = config.theme.clone();
        if let Some(home) = std::env::var_os("HOME") {
            let mut path = std::path::PathBuf::from(home);
            path.push(".orbit_theme");
            if let Ok(contents) = std::fs::read_to_string(path) {
                let read = contents.trim();
                if !read.is_empty() {
                    theme_name = read.to_owned();
                }
            }
        }
        let theme = crate::theme::get_theme(&theme_name);
        let glass = config.glass.clone();

        let mut app = Self {
            config,
            manager,
            workspace_dialog: None,
            search_open: false,
            history_open: false,
            typography_dirty: true,
            theme_dirty: true,
            appearance_dirty: false,
            command_palette_open: false,
            palette_filter: String::new(),
            palette_selected: 0,
            palette_just_opened: false,
            theme_name,
            theme,
            available_themes,
            glass,
            glass_settings_open: false,
            appearance_settings_open: false,
            backend: crate::glass::backend::probe(),
            last_blur_size: None,
            debug_pane_id: None,
            debug_focus_requested: false,
            debug_search_focused: false,
        };
        eprintln!("[ORBIT] glass backend: {}", app.backend.describe());
        app.apply_typography(&creation.egui_ctx);
        app.apply_theme(&creation.egui_ctx);
        app.typography_dirty = false;
        app.theme_dirty = false;
        app.persist_config();
        app
    }

    /// Builds a live workspace runtime from a saved workspace: spawns fresh
    /// PTY sessions for every saved pane, resolving directories safely.
    fn build_runtime(config: &TerminalConfig, ws: &Workspace) -> WorkspaceRuntime<TerminalTab> {
        let mut next_tab_id = 1usize;
        let mut next_pane_id = 1usize;
        let tabs = if ws.tabs.is_empty() {
            vec![TerminalTab::build_default(ws, config)]
        } else {
            ws.tabs
                .iter()
                .map(|saved| TerminalTab::from_saved(saved, ws, config))
                .collect()
        };
        for tab in &tabs {
            next_tab_id = next_tab_id.max(tab.id.saturating_add(1));
            tab.for_each_pane(|pane| next_pane_id = next_pane_id.max(pane.id.saturating_add(1)));
        }
        WorkspaceRuntime::new(ws.clone(), tabs, next_tab_id, next_pane_id)
    }

    fn active_tab_mut(&mut self) -> Option<&mut TerminalTab> {
        let ws = self.manager.active_mut()?;
        ws.tabs.get_mut(ws.active_tab)
    }

    fn active_tab(&self) -> Option<&TerminalTab> {
        let ws = self.manager.active()?;
        ws.tabs.get(ws.active_tab)
    }

    fn drain_pty(&mut self) {
        for ws in &mut self.manager.workspaces {
            for tab in &mut ws.tabs {
                tab.for_each_pane_mut(|pane| pane.drain_pty());
            }
        }
    }

    fn ui_top_bar(&mut self, ui: &mut egui::Ui) {
        let mut close_tab: Option<usize> = None;
        let mut new_tab_requested = false;
        let mut toggle_search = false;
        let mut toggle_history = false;
        ui.horizontal(|ui| {
            let tab_count = self.manager.active().map(|ws| ws.tabs.len()).unwrap_or(0);
            for index in 0..tab_count {
                if self.ui_tab(ui, index) {
                    close_tab = Some(index);
                }
            }

            ui.separator();

            if ui.button("+").on_hover_text("New tab").clicked() {
                new_tab_requested = true;
            }
            if ui.button("H").on_hover_text("Command history").clicked() {
                toggle_history = true;
            }
            if ui.button("F").on_hover_text("Search").clicked() {
                toggle_search = true;
            }

            ui.separator();
            egui::ComboBox::from_label("Theme")
                .selected_text(self.theme_name.clone())
                .show_ui(ui, |ui| {
                    for index in 0..self.available_themes.len() {
                        let name = self.available_themes[index];
                        if ui.selectable_label(name == self.theme_name, name).clicked() {
                            self.theme_name = name.to_string();
                            self.theme = crate::theme::get_theme(&self.theme_name);
                            self.config.theme = self.theme_name.clone();
                            self.theme_dirty = true;
                            self.persist_config();
                            if let Some(home) = std::env::var_os("HOME") {
                                let mut path = std::path::PathBuf::from(home);
                                path.push(".orbit_theme");
                                if let Ok(mut file) = std::fs::File::create(path) {
                                    let _ = writeln!(file, "{}", self.theme_name);
                                }
                            }
                        }
                    }

                    ui.separator();
                    let (status_text, status_color) = self.active_status_label();
                    ui.colored_label(status_color, status_text);
                    ui.label(
                        egui::RichText::new(self.theme.name).color(self.theme.ui.secondary_text),
                    );
                });

            ui.separator();
            ui.label("Typography");
            let font_names = TypographyConfig::available_font_names();
            let selected_font = self.config.typography.resolved_terminal_font_name();
            egui::ComboBox::from_label("Font")
                .selected_text(selected_font.clone())
                .show_ui(ui, |ui| {
                    for name in &font_names {
                        if ui.selectable_label(name == &selected_font, name).clicked() {
                            self.config.typography.terminal_font = name.clone();
                            self.typography_dirty = true;
                            self.persist_config();
                        }
                    }
                });

            if ui
                .add(
                    egui::Slider::new(&mut self.config.typography.terminal_font_size, 8.0..=32.0)
                        .text("Terminal size"),
                )
                .changed()
            {
                self.typography_dirty = true;
                self.persist_config();
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.config.typography.line_spacing, 0.0..=12.0)
                        .text("Line spacing"),
                )
                .changed()
            {
                self.typography_dirty = true;
                self.persist_config();
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.config.typography.character_spacing, 0.0..=8.0)
                        .text("Char spacing"),
                )
                .changed()
            {
                self.typography_dirty = true;
                self.persist_config();
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.config.typography.ui_font_size, 10.0..=24.0)
                        .text("UI size"),
                )
                .changed()
            {
                self.typography_dirty = true;
                self.persist_config();
            }

            ui.separator();

            let appearance_button = ui
                .button("Appearance")
                .on_hover_text("Cursor and appearance settings");
            if appearance_button.clicked() {
                self.appearance_settings_open = !self.appearance_settings_open;
            }
            if self.appearance_settings_open {
                egui::Popup::from_response(&appearance_button)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        self.appearance_settings_ui(ui);
                    });
            }

            let glass_label = if self.glass.enabled {
                "Glass: ON"
            } else {
                "Glass: OFF"
            };
            if ui
                .button(glass_label)
                .on_hover_text("Toggle the glass background (no restart needed)")
                .clicked()
            {
                self.glass.enabled = !self.glass.enabled;
                self.glass_changed();
            }
            if self.glass.enabled {
                let settings_button = ui
                    .button("Settings")
                    .on_hover_text("Glass material settings");
                if settings_button.clicked() {
                    self.glass_settings_open = !self.glass_settings_open;
                }
                if self.glass_settings_open {
                    egui::Popup::from_response(&settings_button)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| {
                            self.glass_settings_ui(ui);
                        });
                }
            }
        });

        if new_tab_requested {
            self.new_tab();
        }
        if toggle_history {
            self.history_open = !self.history_open;
        }
        if toggle_search {
            self.search_open = !self.search_open;
        }
        if let Some(index) = close_tab {
            self.close_tab(index);
        }
    }

    /// One tab chip: rounded background, hover highlight, accent underline on
    /// the active tab and a per-tab close button. Returns `true` when this
    /// tab's close button was clicked.
    fn ui_tab(&mut self, ui: &mut egui::Ui, index: usize) -> bool {
        let (selected, title) = {
            let Some(ws) = self.manager.active() else {
                return false;
            };
            if index >= ws.tabs.len() {
                return false;
            }
            (ws.active_tab == index, ws.tabs[index].title.clone())
        };
        let theme = self.theme.clone();
        let appearance = self.config.appearance.clone();
        let radius = appearance.panel_radius.clamp(0.0, 12.0) as u8;

        let font_id = egui::TextStyle::Body.resolve(ui.style());
        let text_size = ui
            .fonts(|fonts| fonts.layout_no_wrap(title.clone(), font_id.clone(), theme.ui.text))
            .size();
        let close_size = 14.0;
        let height = 26.0;
        let width = text_size.x + if selected { close_size + 16.0 } else { 14.0 };
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width.max(24.0), height), egui::Sense::click());

        if response.clicked() {
            if let Some(ws) = self.manager.active_mut() {
                ws.active_tab = index;
            }
        }

        let hovered = response.hovered();
        let fill = if selected {
            theme.ui.tab_active
        } else if hovered {
            lerp_color(theme.ui.tab_inactive, theme.ui.tab_active, 0.45)
        } else {
            theme.ui.tab_inactive
        };
        ui.painter().rect_filled(rect, radius, fill);
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0_f32, theme.ui.divider),
            egui::StrokeKind::Inside,
        );
        if selected {
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    rect.left_bottom() - egui::vec2(0.0, 2.0),
                    rect.right_bottom(),
                ),
                1.0,
                theme.ui.accent,
            );
        }

        let text_color = if selected {
            theme.ui.text
        } else {
            theme.ui.secondary_text
        };
        ui.painter().text(
            rect.left_top() + egui::vec2(6.0, (height - text_size.y) / 2.0),
            egui::Align2::LEFT_TOP,
            title,
            font_id,
            text_color,
        );

        let mut closed = false;
        if selected || hovered {
            let close_rect = egui::Rect::from_center_size(
                rect.right_center() - egui::vec2(close_size / 2.0 + 4.0, 0.0),
                egui::vec2(close_size, close_size),
            );
            let close_response = ui.interact(
                close_rect,
                ui.id().with("orbit_tab_close").with(index),
                egui::Sense::click(),
            );
            if close_response.hovered() {
                ui.painter()
                    .circle_filled(close_rect.center(), close_size / 2.0, theme.ui.divider);
            }
            ui.painter().text(
                close_rect.center(),
                egui::Align2::CENTER_CENTER,
                "×",
                egui::FontId::proportional(12.0),
                if close_response.hovered() {
                    theme.ui.text
                } else {
                    theme.ui.secondary_text
                },
            );
            if close_response.clicked() {
                closed = true;
            }
        }

        closed
    }

    /// Workspace selector row: one chip per workspace plus a "new workspace"
    /// button. Chips can be right-clicked for rename/duplicate/delete/
    /// reorder/default actions.
    fn ui_workspace_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Workspaces")
                    .small()
                    .color(self.theme.ui.secondary_text),
            );
            ui.separator();
            for index in 0..self.manager.workspaces.len() {
                self.ui_workspace_chip(ui, index);
            }
            ui.separator();
            if ui
                .button("+")
                .on_hover_text("New workspace (Ctrl+Shift+P)")
                .clicked()
            {
                self.open_create_dialog();
            }
        });
    }

    fn ui_workspace_chip(&mut self, ui: &mut egui::Ui, index: usize) {
        if index >= self.manager.workspaces.len() {
            return;
        }
        let (selected, data) = {
            let ws = &self.manager.workspaces[index];
            (index == self.manager.active, ws.data.clone())
        };
        let is_default = data.id == self.manager.default_id;
        let theme = self.theme.clone();
        let appearance = self.config.appearance.clone();
        let radius = appearance.panel_radius.clamp(0.0, 12.0) as u8;

        let label = if is_default {
            format!("★ {} {}", data.icon, data.name)
        } else {
            format!("{} {}", data.icon, data.name)
        };
        let font_id = egui::TextStyle::Body.resolve(ui.style());
        let text_size = ui
            .fonts(|fonts| fonts.layout_no_wrap(label.clone(), font_id.clone(), theme.ui.text))
            .size();
        let height = 24.0;
        let width = text_size.x + 16.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width.max(20.0), height), egui::Sense::click());

        if response.clicked() {
            self.manager.switch_to(index);
            self.persist_config();
        }

        let hovered = response.hovered();
        let fill = if selected {
            theme.ui.tab_active
        } else if hovered {
            lerp_color(theme.ui.tab_inactive, theme.ui.tab_active, 0.45)
        } else {
            theme.ui.tab_inactive
        };
        ui.painter().rect_filled(rect, radius, fill);
        ui.painter().rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0_f32, theme.ui.divider),
            egui::StrokeKind::Inside,
        );
        if selected {
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    rect.left_bottom() - egui::vec2(0.0, 2.0),
                    rect.right_bottom(),
                ),
                1.0,
                theme.ui.accent,
            );
        }
        let text_color = if selected {
            theme.ui.text
        } else {
            theme.ui.secondary_text
        };
        ui.painter().text(
            rect.left_top() + egui::vec2(6.0, (height - text_size.y) / 2.0),
            egui::Align2::LEFT_TOP,
            label,
            font_id,
            text_color,
        );

        response.context_menu(|ui| {
            ui.set_min_width(180.0);
            ui.label(
                egui::RichText::new(&data.name)
                    .strong()
                    .color(theme.ui.secondary_text),
            );
            ui.separator();
            if ui.button("Rename…").clicked() {
                self.open_rename_dialog(index);
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                self.duplicate_workspace(index);
                ui.close();
            }
            if ui.button("Delete").clicked() {
                self.delete_workspace(index);
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(index > 0, egui::Button::new("Move left"))
                .clicked()
            {
                self.manager.move_left(index);
                self.persist_config();
                ui.close();
            }
            if ui
                .add_enabled(
                    index + 1 < self.manager.workspaces.len(),
                    egui::Button::new("Move right"),
                )
                .clicked()
            {
                self.manager.move_right(index);
                self.persist_config();
                ui.close();
            }
            ui.separator();
            if !is_default && ui.button("Set as default").clicked() {
                self.manager.set_default(index);
                self.persist_config();
                ui.close();
            }
        });
    }

    fn open_create_dialog(&mut self) {
        let dir = self
            .manager
            .active()
            .map(|ws| ws.data.working_dir.clone())
            .unwrap_or_else(|| self.config.working_dir.clone());
        self.workspace_dialog = Some(WorkspaceDialog::Create {
            name: String::new(),
            preset: WorkspacePreset::Coding,
            dir: dir.to_string_lossy().into_owned(),
        });
    }

    fn open_rename_dialog(&mut self, index: usize) {
        let Some(ws) = self.manager.workspaces.get(index) else {
            return;
        };
        self.workspace_dialog = Some(WorkspaceDialog::Rename {
            id: ws.data.id.clone(),
            name: ws.data.name.clone(),
        });
    }

    fn ui_workspace_dialog(&mut self, ctx: &egui::Context) {
        let mut collect: Option<(String, WorkspacePreset, String)> = None;
        let mut rename_target: Option<String> = None;
        let mut close = false;

        match &mut self.workspace_dialog {
            Some(WorkspaceDialog::Create { name, preset, dir }) => {
                let mut open = true;
                let mut submit = false;
                let mut closed = false;
                egui::Window::new("New workspace")
                    .open(&mut open)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.set_min_width(360.0);
                        ui.label("Name");
                        ui.text_edit_singleline(name);
                        ui.label("Preset");
                        egui::ComboBox::from_id_salt("workspace_create_preset")
                            .selected_text(preset.label())
                            .show_ui(ui, |ui| {
                                for candidate in WorkspacePreset::CREATABLE {
                                    if ui
                                        .selectable_label(*preset == candidate, candidate.label())
                                        .clicked()
                                    {
                                        *preset = candidate;
                                    }
                                }
                            });
                        ui.label(
                            egui::RichText::new(preset.purpose())
                                .small()
                                .color(self.theme.ui.secondary_text),
                        );
                        ui.label("Directory");
                        ui.text_edit_singleline(dir);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!name.trim().is_empty(), egui::Button::new("Create"))
                                .clicked()
                            {
                                submit = true;
                            }
                            if ui.button("Cancel").clicked() {
                                closed = true;
                            }
                        });
                        ui.label(
                            egui::RichText::new(
                                "Missing directories fall back to your home directory.",
                            )
                            .small()
                            .color(self.theme.ui.secondary_text),
                        );
                    });
                if closed {
                    open = false;
                }
                if submit {
                    collect = Some((name.clone(), *preset, dir.clone()));
                }
                if !open {
                    close = true;
                }
            }
            Some(WorkspaceDialog::Rename { id, name }) => {
                let mut open = true;
                let mut submit = false;
                let mut closed = false;
                egui::Window::new("Rename workspace")
                    .open(&mut open)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.set_min_width(300.0);
                        ui.label("Name");
                        ui.text_edit_singleline(name);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!name.trim().is_empty(), egui::Button::new("Rename"))
                                .clicked()
                            {
                                submit = true;
                            }
                            if ui.button("Cancel").clicked() {
                                closed = true;
                            }
                        });
                    });
                if closed {
                    open = false;
                }
                if submit {
                    rename_target = Some(id.clone());
                }
                if !open {
                    close = true;
                }
            }
            None => return,
        }

        if let Some((name, preset, dir)) = collect {
            self.create_workspace(name, preset, dir);
            close = true;
        }
        if let Some(id) = rename_target {
            if let Some(dialog) = &self.workspace_dialog {
                if let WorkspaceDialog::Rename { name, .. } = dialog {
                    if self.manager.rename(&id, name) {
                        self.persist_config();
                        close = true;
                    }
                }
            }
        }
        if close {
            self.workspace_dialog = None;
        }
    }

    fn create_workspace(&mut self, name: String, preset: WorkspacePreset, dir: String) {
        let working_dir = resolve_dir_input(&dir, &self.config.working_dir);
        let mut ws = Workspace::from_preset(preset, working_dir, new_id("ws"));
        let name = name.trim().to_owned();
        ws.name = if name.is_empty() {
            preset.label().to_owned()
        } else {
            name
        };
        let runtime = Self::build_runtime(&self.config, &ws);
        let _ = self.manager.create(runtime);
        self.persist_config();
    }

    fn duplicate_workspace(&mut self, index: usize) {
        let config = self.config.clone();
        self.manager
            .duplicate(index, |ws| Self::build_runtime(&config, ws));
        self.persist_config();
    }

    fn delete_workspace(&mut self, index: usize) {
        let config = self.config.clone();
        self.manager
            .delete(index, |ws| Self::build_runtime(&config, ws));
        self.persist_config();
    }

    fn palette_items(&self) -> Vec<(String, PaletteAction)> {
        let mut items = Vec::new();
        for (index, ws) in self.manager.workspaces.iter().enumerate() {
            items.push((
                format!("Workspace: Switch to {}", ws.data.name),
                PaletteAction::SwitchToWorkspace(index),
            ));
        }
        items.push((
            "Workspace: Next".to_owned(),
            PaletteAction::App(AppAction::NextWorkspace),
        ));
        items.push((
            "Workspace: Previous".to_owned(),
            PaletteAction::App(AppAction::PreviousWorkspace),
        ));
        items.push(("Workspace: New".to_owned(), PaletteAction::NewWorkspace));
        items.push((
            "Workspace: Rename".to_owned(),
            PaletteAction::RenameWorkspace,
        ));
        items.push((
            "Workspace: Duplicate".to_owned(),
            PaletteAction::DuplicateWorkspace,
        ));
        items.push((
            "Workspace: Delete".to_owned(),
            PaletteAction::DeleteWorkspace,
        ));
        for (index, ws) in self.manager.workspaces.iter().enumerate() {
            items.push((
                format!("Workspace: Set default: {}", ws.data.name),
                PaletteAction::SetDefaultWorkspace(index),
            ));
        }
        items.push(("Workspace: Save".to_owned(), PaletteAction::SaveWorkspace));
        items.push((
            "Terminal: New tab".to_owned(),
            PaletteAction::App(AppAction::NewTab),
        ));
        items.push((
            "Terminal: Close tab".to_owned(),
            PaletteAction::App(AppAction::CloseTab),
        ));
        items.push((
            "Terminal: Next tab".to_owned(),
            PaletteAction::App(AppAction::NextTab),
        ));
        items.push((
            "Terminal: Previous tab".to_owned(),
            PaletteAction::App(AppAction::PreviousTab),
        ));
        items.push((
            "Terminal: Split horizontal".to_owned(),
            PaletteAction::App(AppAction::SplitHorizontal),
        ));
        items.push((
            "Terminal: Split vertical".to_owned(),
            PaletteAction::App(AppAction::SplitVertical),
        ));
        items.push((
            "Terminal: Close pane".to_owned(),
            PaletteAction::App(AppAction::ClosePane),
        ));
        items.push((
            "Terminal: Restart pane".to_owned(),
            PaletteAction::App(AppAction::RestartPane),
        ));
        items.push((
            "Terminal: Toggle search".to_owned(),
            PaletteAction::App(AppAction::ToggleSearch),
        ));
        items.push((
            "Terminal: Toggle history".to_owned(),
            PaletteAction::App(AppAction::ToggleHistory),
        ));
        items
    }

    fn ui_command_palette(&mut self, ctx: &egui::Context) {
        if !self.command_palette_open {
            return;
        }
        let items = self.palette_items();
        let mut open = true;
        let mut action: Option<PaletteAction> = None;
        let mut closed_by_key = false;

        egui::Window::new("Command palette")
            .open(&mut open)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 48.0])
            .collapsible(false)
            .resizable(false)
            .default_width(440.0)
            .show(ctx, |ui| {
                let mut filter = self.palette_filter.clone();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut filter)
                        .desired_width(f32::INFINITY)
                        .hint_text("Type a command… (Ctrl+Shift+P to close)"),
                );
                if self.palette_just_opened {
                    ctx.memory_mut(|memory| memory.request_focus(response.id));
                }

                let filtered: Vec<usize> = items
                    .iter()
                    .enumerate()
                    .filter(|(_, (label, _))| {
                        filter.is_empty() || label.to_lowercase().contains(&filter.to_lowercase())
                    })
                    .map(|(index, _)| index)
                    .collect();

                let arrow_down = ui.input(|input| input.key_pressed(egui::Key::ArrowDown));
                let arrow_up = ui.input(|input| input.key_pressed(egui::Key::ArrowUp));
                if arrow_down && !filtered.is_empty() {
                    self.palette_selected = (self.palette_selected + 1) % filtered.len();
                }
                if arrow_up && !filtered.is_empty() {
                    self.palette_selected =
                        (self.palette_selected + filtered.len() - 1) % filtered.len();
                }
                self.palette_selected = self.palette_selected.min(filtered.len().saturating_sub(1));

                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if enter && !filtered.is_empty() {
                    action = Some(items[filtered[self.palette_selected]].1.clone());
                }

                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (rank, &item_index) in filtered.iter().enumerate() {
                            let (label, item_action) = &items[item_index];
                            let selected = rank == self.palette_selected;
                            if ui
                                .selectable_label(selected, label.clone())
                                .on_hover_text(item_action_label(item_action))
                                .clicked()
                            {
                                action = Some(item_action.clone());
                            }
                        }
                        if filtered.is_empty() {
                            ui.label("No matching commands.");
                        }
                    });

                self.palette_filter = filter;
            });

        if self.palette_just_opened {
            self.palette_just_opened = false;
        }
        if ui_input_escape(ctx) {
            closed_by_key = true;
        }
        if let Some(action) = action {
            self.run_palette_action(action, ctx);
            self.command_palette_open = false;
        } else if !open || closed_by_key {
            self.command_palette_open = false;
        }
    }

    fn run_palette_action(&mut self, action: PaletteAction, ctx: &egui::Context) {
        match action {
            PaletteAction::App(action) => self.run_action(action, ctx),
            PaletteAction::SwitchToWorkspace(index) => {
                self.manager.switch_to(index);
                self.persist_config();
            }
            PaletteAction::NewWorkspace => self.open_create_dialog(),
            PaletteAction::RenameWorkspace => {
                if !self.manager.workspaces.is_empty() {
                    self.open_rename_dialog(self.manager.active);
                }
            }
            PaletteAction::DuplicateWorkspace => self.duplicate_workspace(self.manager.active),
            PaletteAction::DeleteWorkspace => self.delete_workspace(self.manager.active),
            PaletteAction::SetDefaultWorkspace(index) => {
                self.manager.set_default(index);
                self.persist_config();
            }
            PaletteAction::SaveWorkspace => self.persist_config(),
        }
    }

    fn glass_settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.set_min_width(300.0);
        ui.label(egui::RichText::new("Glass material").strong());
        ui.separator();

        changed |= ui
            .add(egui::Slider::new(&mut self.glass.opacity, 0.3..=1.0).text("Opacity"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.glass.tint_opacity, 0.0..=1.0).text("Tint strength"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.glass.blur_strength, 0.0..=10.0).text("Blur strength"))
            .changed();

        ui.horizontal(|ui| {
            ui.label("Tint");
            egui::ComboBox::from_id_salt("glass_tint_combo")
                .selected_text(self.glass.tint.label())
                .show_ui(ui, |ui| {
                    for tint in GlassTint::ALL {
                        if ui
                            .selectable_label(self.glass.tint == tint, tint.label())
                            .clicked()
                        {
                            self.glass.tint = tint;
                            changed = true;
                        }
                    }
                });
        });

        if self.glass.tint == GlassTint::Custom {
            let mut rgb = self.glass.custom_tint;
            changed |= ui
                .add(egui::Slider::new(&mut rgb[0], 0..=255).text("R"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut rgb[1], 0..=255).text("G"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut rgb[2], 0..=255).text("B"))
                .changed();
            self.glass.custom_tint = rgb;
        }

        if changed {
            self.glass_changed();
        }

        ui.separator();
        ui.label(
            egui::RichText::new(self.backend.describe())
                .small()
                .color(self.theme.ui.secondary_text),
        );
        ui.label(
            egui::RichText::new(
                "Blur strength only applies on compositors that expose window blur.",
            )
            .small()
            .color(self.theme.ui.secondary_text),
        );
    }

    fn glass_changed(&mut self) {
        self.config.glass = self.glass.clone();
        self.persist_config();
    }

    fn appearance_settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.set_min_width(340.0);
        ui.label(egui::RichText::new("Appearance").strong());
        ui.label(
            egui::RichText::new("Applies immediately; no restart needed.")
                .small()
                .color(self.theme.ui.secondary_text),
        );
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Cursor style");
            egui::ComboBox::from_id_salt("appearance_cursor_style")
                .selected_text(self.config.appearance.cursor_style.label())
                .show_ui(ui, |ui| {
                    for style in CursorStyle::ALL {
                        if ui
                            .selectable_label(
                                self.config.appearance.cursor_style == style,
                                style.label(),
                            )
                            .clicked()
                        {
                            self.config.appearance.cursor_style = style;
                            changed = true;
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Cursor blink");
            changed |= ui
                .checkbox(&mut self.config.appearance.cursor_blink, "Blink")
                .changed();
            if self.config.appearance.cursor_blink {
                egui::ComboBox::from_id_salt("appearance_blink_speed")
                    .selected_text(self.config.appearance.cursor_blink_speed.label())
                    .show_ui(ui, |ui| {
                        for speed in crate::config::CursorBlinkSpeed::ALL {
                            if ui
                                .selectable_label(
                                    self.config.appearance.cursor_blink_speed == speed,
                                    speed.label(),
                                )
                                .clicked()
                            {
                                self.config.appearance.cursor_blink_speed = speed;
                                changed = true;
                            }
                        }
                    });
            }
        });
        ui.horizontal(|ui| {
            ui.label("Cursor color");
            egui::ComboBox::from_id_salt("appearance_cursor_color_mode")
                .selected_text(self.config.appearance.cursor_color_mode.label())
                .show_ui(ui, |ui| {
                    for mode in CursorColorMode::ALL {
                        if ui
                            .selectable_label(
                                self.config.appearance.cursor_color_mode == mode,
                                mode.label(),
                            )
                            .clicked()
                        {
                            self.config.appearance.cursor_color_mode = mode;
                            changed = true;
                        }
                    }
                });
        });
        if self.config.appearance.cursor_color_mode == CursorColorMode::Custom {
            let mut rgb = self.config.appearance.cursor_custom_color;
            changed |= ui
                .add(egui::Slider::new(&mut rgb[0], 0..=255).text("R"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut rgb[1], 0..=255).text("G"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut rgb[2], 0..=255).text("B"))
                .changed();
            self.config.appearance.cursor_custom_color = rgb;
        }
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.appearance.cursor_thickness, 1.0..=8.0)
                    .text("Cursor thickness"),
            )
            .changed();

        ui.separator();
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.appearance.panel_radius, 0.0..=16.0)
                    .text("Panel radius"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.appearance.border_width, 0.0..=4.0)
                    .text("Border width"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.appearance.border_opacity, 0.0..=1.0)
                    .text("Border opacity"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.appearance.spacing_scale, 0.8..=1.4)
                    .text("Spacing scale"),
            )
            .changed();

        if changed {
            self.appearance_dirty = true;
            self.persist_config();
        }
    }

    /// Background color for the central area, glass-aware.
    fn glass_background(&self) -> egui::Color32 {
        if self.glass.enabled {
            crate::glass::glass_fill(
                self.theme.ui.background,
                self.glass.tint,
                self.glass.custom_tint,
                self.glass.tint_opacity,
                self.glass.opacity,
            )
        } else {
            self.theme.ui.background
        }
    }

    /// Panel (chrome) color for bars and side panels, glass-aware.
    fn glass_panel(&self) -> egui::Color32 {
        if self.glass.enabled {
            crate::glass::glass_fill(
                self.theme.ui.panel,
                self.glass.tint,
                self.glass.custom_tint,
                self.glass.tint_opacity,
                self.glass.opacity,
            )
        } else {
            self.theme.ui.panel
        }
    }

    /// Keeps the compositor blur region in sync with the window size. Only
    /// does real work when a native blur backend is active, and only when the
    /// window actually changed size.
    fn update_glass_backdrop(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !self.glass.enabled || self.glass.blur_strength <= 0.0 {
            return;
        }
        if !self.backend.x11_blur_available() {
            return;
        }
        let size = ctx.screen_rect().size();
        if self.last_blur_size == Some(size) {
            return;
        }
        self.last_blur_size = Some(size);
        let Ok(handle) = frame.window_handle() else {
            return;
        };
        let Some(window) = x11_window_id(&handle) else {
            return;
        };
        let scale = ctx.pixels_per_point();
        let width = (size.x * scale).round().max(1.0) as u32;
        let height = (size.y * scale).round().max(1.0) as u32;
        if let Err(error) = apply_x11_blur_region(window, 0, 0, width, height) {
            eprintln!("[ORBIT] failed to apply blur region: {error}");
        }
    }

    fn ui_search_bar(&mut self, ui: &mut egui::Ui) {
        if !self.search_open {
            return;
        }

        let active_pane_id = self.active_tab().map(|tab| tab.active_pane);
        let mut close_search = false;
        let mut focus_requested = false;
        let debug_env = std::env::var_os("ORBIT_DEBUG_EVENTS").is_some();
        let should_autofocus = debug_env && !self.debug_search_focused;
        ui.horizontal(|ui| {
            ui.label("Search");
            if let Some(pane) = self.active_pane_mut() {
                // avoid borrowing pane across UI input check that mutates self; use a local copy and assign back
                let mut local_query = pane.search_query.clone();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut local_query)
                        .desired_width(240.0)
                        .hint_text("visible terminal text"),
                );
                if response.changed() {
                    pane.search_query = local_query.clone();
                    pane.search_current = None;
                }
                if should_autofocus {
                    ui.memory_mut(|m| m.request_focus(response.id));
                    focus_requested = true;
                }
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    close_search = true;
                }
                let count = pane.search_match_count();
                ui.label(format!("{count} visible matches"));
                if ui.button("Next").on_hover_text("Find next (F3)").clicked() {
                    pane.find_next_match();
                }
                if ui
                    .button("Prev")
                    .on_hover_text("Find previous (Shift+F3)")
                    .clicked()
                {
                    pane.find_previous_match();
                }
            }
            if ui.button("Clear").clicked() {
                if let Some(pane) = self.active_pane_mut() {
                    pane.search_query.clear();
                    pane.search_current = None;
                }
            }
            if let Some(id) = active_pane_id {
                ui.label(format!("pane {id}"));
            }
        });

        if close_search {
            self.search_open = false;
        }
        if focus_requested {
            self.debug_search_focused = true;
        }
    }

    fn ui_history_panel(&mut self, ctx: &egui::Context) {
        if !self.history_open {
            return;
        }

        let border_color =
            crate::glass::with_alpha(self.theme.ui.border, self.config.appearance.border_opacity);
        let frame = egui::Frame::new()
            .fill(self.glass_panel())
            .corner_radius(self.config.appearance.panel_radius.clamp(0.0, 16.0) as u8)
            .inner_margin(egui::Margin::same(10))
            .stroke(egui::Stroke::new(
                self.config.appearance.border_width.clamp(0.0, 4.0),
                border_color,
            ));

        egui::SidePanel::right("orbit_history")
            .resizable(true)
            .default_width(280.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.heading("Command History");
                ui.separator();

                let Some(tab) = self.active_tab_mut() else {
                    return;
                };
                let Some(pane) = tab.active_pane_mut() else {
                    return;
                };

                // Filter input for history
                ui.horizontal(|ui| {
                    ui.label("Filter:");
                    ui.add(
                        egui::TextEdit::singleline(&mut pane.history_filter).desired_width(160.0),
                    );
                    if ui.button("Clear").clicked() {
                        pane.history_filter.clear();
                    }
                });

                if pane.history.is_empty() {
                    ui.label("No commands captured yet.");
                    return;
                }

                let filter = pane.history_filter.to_lowercase();
                for entry in pane
                    .history
                    .iter()
                    .rev()
                    .filter(|e| {
                        if filter.is_empty() {
                            true
                        } else {
                            e.text.to_lowercase().contains(&filter)
                        }
                    })
                    .take(100)
                {
                    let age = entry
                        .submitted_at
                        .elapsed()
                        .map(|elapsed| format_age(elapsed))
                        .unwrap_or_else(|_| "now".to_owned());
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(&entry.text);
                        ui.label(age);
                    });
                }
            });
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context, terminal_has_focus: bool) {
        let events = ctx.input(|input| input.events.clone());

        for event in events {
            if let Some(action) = action_from_event(&event) {
                self.run_action(action, ctx);
                continue;
            }

            if !terminal_has_focus {
                continue;
            }

            if self.search_open {
                if matches!(
                    event,
                    egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Key { .. }
                ) {
                    continue;
                }
            }

            let Some(pane) = self.active_pane_mut() else {
                continue;
            };

            let Some(bytes) = event_to_terminal_bytes(&event) else {
                continue;
            };

            pane.record_input_event(&event);
            pane.write_to_pty(&bytes);
        }
    }

    fn paint_active_tab(&mut self, ui: &mut egui::Ui) -> egui::Response {
        // Clone theme before mutably borrowing workspaces to avoid borrow conflicts
        let theme = self.theme.clone();
        let typography = self.config.typography.clone();
        let glass = self.glass.clone();
        let appearance = self.config.appearance.clone();
        let Some(ws) = self.manager.active_mut() else {
            let (_, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
            return response;
        };
        let Some(tab) = ws.tabs.get_mut(ws.active_tab) else {
            let (_, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
            return response;
        };

        let response = tab.paint(ui, &theme, &typography, &glass, &appearance);
        if std::env::var_os("ORBIT_DEBUG_EVENTS").is_some() {
            self.debug_pane_id = Some(response.id);
        }
        response
    }

    fn active_status_label(&self) -> (String, egui::Color32) {
        match self.active_tab() {
            Some(tab) => match tab.active_pane() {
                Some(pane) if pane.exited => ("shell exited".to_owned(), self.theme.status.error),
                Some(pane) if pane.pty.is_err() => {
                    ("pty error".to_owned(), self.theme.status.error)
                }
                Some(_) => ("running".to_owned(), self.theme.status.success),
                None => ("no active pane".to_owned(), self.theme.status.warning),
            },
            None => ("no tabs".to_owned(), self.theme.status.warning),
        }
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let theme = self.theme.clone();
        ctx.style_mut(|style| {
            style.visuals = if is_light_color(theme.ui.background) {
                egui::Visuals::light()
            } else {
                egui::Visuals::dark()
            };

            style.visuals.window_fill = theme.ui.background;
            style.visuals.panel_fill = theme.ui.panel;
            style.visuals.extreme_bg_color = theme.ui.background;
            style.visuals.faint_bg_color = theme.ui.panel;
            style.visuals.code_bg_color = theme.ui.panel;
            style.visuals.override_text_color = Some(theme.ui.text);
            style.visuals.hyperlink_color = theme.ui.accent;
            style.visuals.selection.bg_fill = theme.terminal.selection_bg;
            style.visuals.selection.stroke.color = theme
                .terminal
                .selection_fg
                .unwrap_or(theme.terminal.foreground);

            style.visuals.widgets.noninteractive.bg_fill = theme.ui.background;
            style.visuals.widgets.noninteractive.fg_stroke.color = theme.ui.divider;
            style.visuals.widgets.noninteractive.bg_stroke.color = theme.ui.divider;

            style.visuals.widgets.inactive.bg_fill = theme.ui.tab_inactive;
            style.visuals.widgets.inactive.fg_stroke.color = theme.ui.secondary_text;
            style.visuals.widgets.inactive.bg_stroke.color = theme.ui.divider;

            style.visuals.widgets.hovered.bg_fill = theme.ui.tab_active;
            style.visuals.widgets.hovered.fg_stroke.color = theme.ui.text;
            style.visuals.widgets.hovered.bg_stroke.color = theme.ui.accent;

            style.visuals.widgets.active.bg_fill = theme.ui.tab_active;
            style.visuals.widgets.active.fg_stroke.color = theme.ui.text;
            style.visuals.widgets.active.bg_stroke.color = theme.ui.accent;

            style.visuals.window_stroke.color = theme.ui.border;
            style.visuals.window_stroke.width = 1.0;

            let scale = self.config.appearance.spacing_scale.clamp(0.8, 1.4);
            style.spacing.item_spacing = egui::vec2(8.0, 6.0) * scale;
            style.spacing.button_padding = egui::vec2(6.0, 3.0) * scale;
            style.spacing.interact_size.y = 24.0 * scale;
            style.spacing.window_margin = egui::Margin::same((8.0 * scale).round() as i8);
        });
    }

    fn apply_typography(&self, ctx: &egui::Context) {
        let typography = self.config.typography.clone();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut fonts = egui::FontDefinitions::default();
            typography.install_for_egui(&mut fonts);
            ctx.set_fonts(fonts);

            ctx.all_styles_mut(|style| {
                style.text_styles.insert(
                    egui::TextStyle::Body,
                    egui::FontId::new(typography.ui_font_size, egui::FontFamily::Proportional),
                );
                style.text_styles.insert(
                    egui::TextStyle::Monospace,
                    egui::FontId::new(typography.terminal_font_size, egui::FontFamily::Monospace),
                );
                style.text_styles.insert(
                    egui::TextStyle::Button,
                    egui::FontId::new(typography.ui_font_size, egui::FontFamily::Proportional),
                );
                style.text_styles.insert(
                    egui::TextStyle::Small,
                    egui::FontId::new(
                        (typography.ui_font_size - 1.0).max(8.0),
                        egui::FontFamily::Proportional,
                    ),
                );
            });
        }));

        if result.is_err() {
            eprintln!("[ORBIT] failed to apply typography change; keeping the previous font setup");
        }
    }

    fn persist_config(&mut self) {
        self.sync_workspaces_to_config();
        let _ = self.config.save();
    }

    /// Mirrors the live workspace state (order, metadata and terminal layouts)
    /// into the persisted config so the next save captures it.
    fn sync_workspaces_to_config(&mut self) {
        self.config.workspaces = self
            .manager
            .workspaces
            .iter()
            .map(|rt| {
                let mut data = rt.data.clone();
                data.active_tab = rt.active_tab;
                data.tabs = rt.tabs.iter().map(|tab| tab.to_saved()).collect();
                data
            })
            .collect();
        self.config.active_workspace = self.manager.active_id().unwrap_or_default().to_owned();
        self.config.default_workspace = self.manager.default_id.clone();
    }

    fn run_action(&mut self, action: AppAction, ctx: &egui::Context) {
        match action {
            AppAction::NewTab => self.new_tab(),
            AppAction::CloseTab => self.close_active_tab(),
            AppAction::NextTab => self.select_next_tab(),
            AppAction::PreviousTab => self.select_previous_tab(),
            AppAction::SplitHorizontal => self.split_active_pane(SplitAxis::Horizontal),
            AppAction::SplitVertical => self.split_active_pane(SplitAxis::Vertical),
            AppAction::ClosePane => self.close_active_pane(),
            AppAction::RestartPane => self.restart_active_pane(),
            AppAction::ToggleSearch => {
                self.search_open = !self.search_open;
                self.debug_search_focused = false;
            }
            AppAction::ToggleHistory => self.history_open = !self.history_open,
            AppAction::CopySelection => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.copy_selection(ctx);
                }
            }
            AppAction::Paste => {
                // Rely on eframe/egui to generate an Event::Paste when the OS clipboard is pasted.
                // Ctrl+Shift+V mapping exists but actual clipboard read is performed by egui and
                // delivered as Event::Paste which is already handled in handle_keyboard.
            }
            AppAction::FindNext => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.find_next_match();
                }
            }
            AppAction::FindPrevious => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.find_previous_match();
                }
            }
            AppAction::NextPane => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.focus_next_pane();
                }
            }
            AppAction::PreviousPane => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.focus_previous_pane();
                }
            }
            AppAction::PageUp => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.adjust_scrollback(20);
                }
            }
            AppAction::PageDown => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.adjust_scrollback(-20);
                }
            }
            AppAction::NextWorkspace => {
                self.manager.switch_next();
                self.persist_config();
            }
            AppAction::PreviousWorkspace => {
                self.manager.switch_previous();
                self.persist_config();
            }
            AppAction::ToggleCommandPalette => {
                self.command_palette_open = !self.command_palette_open;
                self.palette_just_opened = self.command_palette_open;
                self.palette_selected = 0;
                self.palette_filter.clear();
            }
        }
    }

    fn new_tab(&mut self) {
        let config = self.config.clone();
        let (tab_id, pane_id, dir) = {
            let Some(ws) = self.manager.active_mut() else {
                return;
            };
            let dir = resolve_working_dir(&ws.data.working_dir, &config.working_dir);
            let tab_id = ws.next_tab_id;
            let pane_id = ws.next_pane_id;
            ws.next_tab_id += 1;
            ws.next_pane_id += 1;
            (tab_id, pane_id, dir)
        };
        let pane = TerminalPane::new(
            pane_id,
            &config,
            config.initial_grid,
            dir,
            format!("shell {pane_id}"),
        );
        let tab = TerminalTab {
            id: tab_id,
            title: format!("shell {tab_id}"),
            panes: PaneLayout::Single(pane),
            active_pane: pane_id,
        };
        if let Some(ws) = self.manager.active_mut() {
            ws.tabs.push(tab);
            ws.active_tab = ws.tabs.len() - 1;
        }
        self.persist_config();
    }

    fn close_active_tab(&mut self) {
        let last = self.manager.active().is_some_and(|ws| ws.tabs.len() <= 1);
        if last {
            self.restart_active_pane();
            return;
        }
        if let Some(ws) = self.manager.active_mut() {
            ws.tabs.remove(ws.active_tab);
            ws.active_tab = ws.active_tab.min(ws.tabs.len() - 1);
        }
        self.persist_config();
    }

    /// Closes the tab at `index` (used by the per-tab close button). Closing
    /// the last tab restarts its pane instead, like `close_active_tab`.
    fn close_tab(&mut self, index: usize) {
        let last = self.manager.active().is_some_and(|ws| ws.tabs.len() <= 1);
        if last {
            self.restart_active_pane();
            return;
        }
        if let Some(ws) = self.manager.active_mut() {
            if index < ws.tabs.len() {
                ws.tabs.remove(index);
                if index < ws.active_tab {
                    ws.active_tab -= 1;
                }
                ws.active_tab = ws.active_tab.min(ws.tabs.len() - 1);
            }
        }
        self.persist_config();
    }

    fn select_next_tab(&mut self) {
        if let Some(ws) = self.manager.active_mut() {
            if ws.tabs.is_empty() {
                return;
            }
            ws.active_tab = (ws.active_tab + 1) % ws.tabs.len();
        }
    }

    fn select_previous_tab(&mut self) {
        if let Some(ws) = self.manager.active_mut() {
            if ws.tabs.is_empty() {
                return;
            }
            ws.active_tab = if ws.active_tab == 0 {
                ws.tabs.len() - 1
            } else {
                ws.active_tab - 1
            };
        }
    }

    fn split_active_pane(&mut self, axis: SplitAxis) {
        let config = self.config.clone();
        let (pane_id, dir) = {
            let Some(ws) = self.manager.active_mut() else {
                return;
            };
            let dir = ws
                .tabs
                .get(ws.active_tab)
                .and_then(|tab| tab.active_pane())
                .map(|pane| pane.working_dir.clone())
                .unwrap_or_else(|| ws.data.working_dir.clone());
            let pane_id = ws.next_pane_id;
            ws.next_pane_id += 1;
            (pane_id, dir)
        };
        let Some(ws) = self.manager.active_mut() else {
            return;
        };
        let Some(tab) = ws.tabs.get_mut(ws.active_tab) else {
            return;
        };
        tab.split_active(axis, pane_id, &config, dir);
        self.persist_config();
    }

    fn close_active_pane(&mut self) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        tab.close_active_pane();
        self.persist_config();
    }

    fn restart_active_pane(&mut self) {
        let config = self.config.clone();
        let Some(pane) = self.active_pane_mut() else {
            return;
        };
        pane.restart(&config);
        self.persist_config();
    }

    fn active_pane_mut(&mut self) -> Option<&mut TerminalPane> {
        let ws = self.manager.active_mut()?;
        let tab = ws.tabs.get_mut(ws.active_tab)?;
        tab.active_pane_mut()
    }
}

impl TerminalTab {
    fn for_each_pane_mut(&mut self, mut f: impl FnMut(&mut TerminalPane)) {
        match &mut self.panes {
            PaneLayout::Single(pane) => f(pane),
            PaneLayout::Split { first, second, .. } => {
                f(first);
                f(second);
            }
        }
    }

    fn for_each_pane(&self, mut f: impl FnMut(&TerminalPane)) {
        match &self.panes {
            PaneLayout::Single(pane) => f(pane),
            PaneLayout::Split { first, second, .. } => {
                f(first);
                f(second);
            }
        }
    }

    /// Builds a live tab from a saved one, resolving each pane's working
    /// directory safely (pane dir → workspace dir → config dir → home).
    fn from_saved(saved: &SavedTab, ws: &Workspace, config: &TerminalConfig) -> Self {
        let panes = Self::saved_layout(&saved.panes, ws, config);
        Self {
            id: saved.id,
            title: saved.title.clone(),
            panes,
            active_pane: saved.active_pane,
        }
    }

    fn saved_layout(
        saved: &SavedPaneLayout,
        ws: &Workspace,
        config: &TerminalConfig,
    ) -> PaneLayout {
        match saved {
            SavedPaneLayout::Single(pane) => PaneLayout::Single(Self::saved_pane(pane, ws, config)),
            SavedPaneLayout::Split {
                axis,
                first,
                second,
            } => PaneLayout::Split {
                axis: *axis,
                first: Self::saved_pane(first, ws, config),
                second: Self::saved_pane(second, ws, config),
            },
        }
    }

    fn saved_pane(pane: &SavedPane, ws: &Workspace, config: &TerminalConfig) -> TerminalPane {
        let dir = if pane.working_dir.is_dir() {
            pane.working_dir.clone()
        } else {
            resolve_working_dir(&ws.working_dir, &config.working_dir)
        };
        TerminalPane::new(
            pane.id,
            config,
            config.initial_grid,
            dir,
            pane.title.clone(),
        )
    }

    /// Builds a default single-pane tab for a workspace without saved tabs.
    fn build_default(ws: &Workspace, config: &TerminalConfig) -> Self {
        let dir = resolve_working_dir(&ws.working_dir, &config.working_dir);
        let pane = TerminalPane::new(1, config, config.initial_grid, dir, "shell 1".to_owned());
        Self {
            id: 1,
            title: "shell 1".to_owned(),
            panes: PaneLayout::Single(pane),
            active_pane: 1,
        }
    }

    /// Serializes the live tab into the persisted model.
    fn to_saved(&self) -> SavedTab {
        SavedTab {
            id: self.id,
            title: self.title.clone(),
            active_pane: self.active_pane,
            panes: Self::layout_to_saved(&self.panes),
        }
    }

    fn layout_to_saved(layout: &PaneLayout) -> SavedPaneLayout {
        match layout {
            PaneLayout::Single(pane) => SavedPaneLayout::Single(Self::pane_to_saved(pane)),
            PaneLayout::Split {
                axis,
                first,
                second,
            } => SavedPaneLayout::Split {
                axis: *axis,
                first: Self::pane_to_saved(first),
                second: Self::pane_to_saved(second),
            },
        }
    }

    fn pane_to_saved(pane: &TerminalPane) -> SavedPane {
        SavedPane {
            id: pane.id,
            title: pane.title.clone(),
            working_dir: pane.working_dir.clone(),
        }
    }

    fn active_pane(&self) -> Option<&TerminalPane> {
        match &self.panes {
            PaneLayout::Single(pane) => Some(pane),
            PaneLayout::Split { first, second, .. } => {
                if first.id == self.active_pane {
                    Some(first)
                } else if second.id == self.active_pane {
                    Some(second)
                } else {
                    None
                }
            }
        }
    }

    fn active_pane_mut(&mut self) -> Option<&mut TerminalPane> {
        match &mut self.panes {
            PaneLayout::Single(pane) => Some(pane),
            PaneLayout::Split { first, second, .. } => {
                if first.id == self.active_pane {
                    Some(first)
                } else if second.id == self.active_pane {
                    Some(second)
                } else {
                    None
                }
            }
        }
    }

    fn paint(
        &mut self,
        ui: &mut egui::Ui,
        theme: &crate::theme::Theme,
        typography: &TypographyConfig,
        glass: &GlassConfig,
        appearance: &AppearanceConfig,
    ) -> egui::Response {
        match &mut self.panes {
            PaneLayout::Single(pane) => pane.paint(
                ui,
                self.active_pane == pane.id,
                theme,
                typography,
                glass,
                appearance,
            ),
            PaneLayout::Split {
                axis,
                first,
                second,
            } => match axis {
                SplitAxis::Horizontal => {
                    let available = ui.available_size();
                    let gap = 6.0 * appearance.spacing_scale.clamp(0.8, 1.4);
                    let half_height = (available.y - gap).max(0.0) / 2.0;
                    let first_response = ui
                        .allocate_ui(egui::vec2(available.x, half_height), |ui| {
                            first.paint(
                                ui,
                                self.active_pane == first.id,
                                theme,
                                typography,
                                glass,
                                appearance,
                            )
                        })
                        .inner;
                    ui.add_space(gap);
                    let second_response = ui
                        .allocate_ui(ui.available_size(), |ui| {
                            second.paint(
                                ui,
                                self.active_pane == second.id,
                                theme,
                                typography,
                                glass,
                                appearance,
                            )
                        })
                        .inner;

                    if first_response.clicked() {
                        self.active_pane = first.id;
                    }
                    if second_response.clicked() {
                        self.active_pane = second.id;
                    }

                    if second_response.has_focus() {
                        second_response
                    } else {
                        first_response
                    }
                }
                SplitAxis::Vertical => {
                    let available = ui.available_size();
                    let gap = 6.0 * appearance.spacing_scale.clamp(0.8, 1.4);
                    let half_width = (available.x - gap).max(0.0) / 2.0;
                    let mut response = None;
                    ui.horizontal(|ui| {
                        let first_response = ui
                            .allocate_ui(egui::vec2(half_width, available.y), |ui| {
                                first.paint(
                                    ui,
                                    self.active_pane == first.id,
                                    theme,
                                    typography,
                                    glass,
                                    appearance,
                                )
                            })
                            .inner;
                        ui.add_space(gap);
                        let second_response = ui
                            .allocate_ui(ui.available_size(), |ui| {
                                second.paint(
                                    ui,
                                    self.active_pane == second.id,
                                    theme,
                                    typography,
                                    glass,
                                    appearance,
                                )
                            })
                            .inner;

                        if first_response.clicked() {
                            self.active_pane = first.id;
                        }
                        if second_response.clicked() {
                            self.active_pane = second.id;
                        }

                        response = Some(if second_response.has_focus() {
                            second_response
                        } else {
                            first_response
                        });
                    });
                    response.expect("split pane response")
                }
            },
        }
    }

    fn split_active(
        &mut self,
        axis: SplitAxis,
        pane_id: usize,
        config: &TerminalConfig,
        working_dir: PathBuf,
    ) {
        let PaneLayout::Single(existing) = &self.panes else {
            self.active_pane = self.inactive_pane_id().unwrap_or(self.active_pane);
            return;
        };

        let grid = existing.last_grid;
        let title = existing.title.clone();
        let new_pane = TerminalPane::new(pane_id, config, grid, working_dir.clone(), title);
        let old = match std::mem::replace(
            &mut self.panes,
            PaneLayout::Single(TerminalPane::placeholder(pane_id + 1)),
        ) {
            PaneLayout::Single(pane) => pane,
            PaneLayout::Split { .. } => unreachable!(),
        };

        self.panes = PaneLayout::Split {
            axis,
            first: old,
            second: new_pane,
        };
        self.active_pane = pane_id;
    }

    fn close_active_pane(&mut self) {
        let PaneLayout::Split { first, second, .. } = &self.panes else {
            return;
        };

        let keep_first = second.id == self.active_pane;
        let keep_id = if keep_first { first.id } else { second.id };

        let replacement = match std::mem::replace(
            &mut self.panes,
            PaneLayout::Single(TerminalPane::placeholder(keep_id)),
        ) {
            PaneLayout::Split { first, second, .. } => {
                if keep_first {
                    PaneLayout::Single(first)
                } else {
                    PaneLayout::Single(second)
                }
            }
            PaneLayout::Single(pane) => PaneLayout::Single(pane),
        };

        self.panes = replacement;
        self.active_pane = keep_id;
    }

    fn inactive_pane_id(&self) -> Option<usize> {
        match &self.panes {
            PaneLayout::Single(_) => None,
            PaneLayout::Split { first, second, .. } => {
                if first.id == self.active_pane {
                    Some(second.id)
                } else {
                    Some(first.id)
                }
            }
        }
    }

    fn focus_next_pane(&mut self) {
        match &self.panes {
            PaneLayout::Single(_) => {}
            PaneLayout::Split { first, second, .. } => {
                self.active_pane = if self.active_pane == first.id {
                    second.id
                } else {
                    first.id
                };
            }
        }
    }

    fn focus_previous_pane(&mut self) {
        // same behavior as focus_next_pane (toggle)
        self.focus_next_pane();
    }
}

impl TerminalPane {
    fn new(
        id: usize,
        config: &TerminalConfig,
        grid: TerminalGrid,
        working_dir: PathBuf,
        title: String,
    ) -> Self {
        let mut pane_config = config.clone();
        pane_config.initial_grid = grid;
        pane_config.working_dir = working_dir.clone();
        let terminal = TerminalState::new(grid, pane_config.scrollback_lines);
        let pty = PtySession::spawn(pane_config).map_err(|err| err.to_string());

        Self {
            id,
            title,
            working_dir,
            terminal,
            pty,
            last_grid: grid,
            scrollback_rows: 0,
            selection: None,
            search_query: String::new(),
            search_current: None,
            current_command: String::new(),
            history: Vec::new(),
            history_filter: String::new(),
            exited: false,
        }
    }

    fn placeholder(id: usize) -> Self {
        let grid = TerminalGrid { rows: 1, cols: 1 };
        Self {
            id,
            title: String::new(),
            working_dir: PathBuf::new(),
            terminal: TerminalState::new(grid, 0),
            pty: Err("placeholder".to_owned()),
            last_grid: grid,
            scrollback_rows: 0,
            selection: None,
            search_query: String::new(),
            search_current: None,
            current_command: String::new(),
            history: Vec::new(),
            history_filter: String::new(),
            exited: true,
        }
    }

    fn restart(&mut self, config: &TerminalConfig) {
        let grid = self.last_grid;
        self.terminal = TerminalState::new(grid, config.scrollback_lines);
        let mut pane_config = config.clone();
        pane_config.initial_grid = grid;
        pane_config.working_dir = self.working_dir.clone();
        self.pty = PtySession::spawn(pane_config).map_err(|err| err.to_string());
        self.scrollback_rows = 0;
        self.selection = None;
        self.search_current = None;
        self.current_command.clear();
        self.exited = false;
    }

    fn drain_pty(&mut self) {
        let Ok(pty) = &self.pty else {
            return;
        };

        while let Ok(command) = pty.output_rx().try_recv() {
            match command {
                PtyCommand::Output(bytes) => self.terminal.process(&bytes),
                PtyCommand::Exited(status) => {
                    self.exited = true;
                    let message = format!(
                        "\r\n[ORBIT] shell exited: {status}. Press Ctrl+Shift+R to restart.\r\n"
                    );
                    self.terminal.process(message.as_bytes());
                }
                PtyCommand::Error(error) => {
                    let message = format!("\r\n[ORBIT] PTY error: {error}\r\n");
                    self.terminal.process(message.as_bytes());
                }
            }
        }
    }

    fn paint(
        &mut self,
        ui: &mut egui::Ui,
        is_active: bool,
        theme: &crate::theme::Theme,
        typography: &TypographyConfig,
        glass: &GlassConfig,
        appearance: &AppearanceConfig,
    ) -> egui::Response {
        let available = ui.available_size();
        let cell_width = typography.cell_width().max(1.0);
        let cell_height = typography.cell_height().max(1.0);
        self.resize_if_needed(available, typography);

        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());
        if response.clicked() {
            response.request_focus();
        }

        let scroll_delta = ui.input(|input| input.smooth_scroll_delta.y + input.raw_scroll_delta.y);
        if response.hovered() && scroll_delta.abs() > 0.0 {
            self.adjust_scrollback((scroll_delta / cell_height).round() as isize);
        }

        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                let cell = self.pointer_to_cell(pos, rect, cell_width, cell_height);
                self.selection = Some(Selection {
                    anchor: cell,
                    focus: cell,
                });
            }
        } else if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let cell = self.pointer_to_cell(pos, rect, cell_width, cell_height);
                if let Some(selection) = self.selection.as_mut() {
                    selection.focus = cell;
                }
            }
        }

        let painter = ui.painter_at(rect);
        let border = if is_active {
            theme.ui.accent
        } else {
            theme.ui.border
        };
        // Glass material layer: translucent, theme-tinted. It is painted
        // *below* every glyph; the cells/cursor/selection are painted on top
        // with opaque theme colors so text stays sharp.
        let fill = if glass.enabled {
            crate::glass::glass_fill(
                theme.terminal.background,
                glass.tint,
                glass.custom_tint,
                glass.tint_opacity,
                glass.opacity,
            )
        } else {
            theme.terminal.background
        };
        let radius = appearance.panel_radius.clamp(0.0, 16.0);
        let border_width = appearance.border_width.clamp(0.0, 4.0);
        let border_color = crate::glass::with_alpha(border, appearance.border_opacity);
        painter.rect_filled(rect, radius, fill);
        if border_width > 0.0 {
            painter.rect_stroke(
                rect,
                radius,
                egui::Stroke::new(border_width, border_color),
                egui::StrokeKind::Inside,
            );
        }
        if glass.enabled {
            // Subtle top edge highlight on the material.
            painter.line_segment(
                [
                    rect.left_top() + egui::vec2(1.0, 1.0),
                    rect.right_top() + egui::vec2(-1.0, 1.0),
                ],
                egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(20)),
            );
        }

        let font_id = typography.terminal_font_id();
        let top_left = rect.left_top() + egui::vec2(10.0, 8.0);
        let rows = self.terminal.visible_rows();
        let selection = self.selection.map(|selection| selection.normalized());
        let search_query = self.search_query.clone();
        let search_len = search_query.chars().count();

        // The cursor is only meaningful on the live screen. When scrolled back
        // the grid offsets no longer line up with `cursor_position()`, and the
        // terminal may have hidden the cursor via an escape sequence.
        let cursor_visible = response.has_focus()
            && is_active
            && !self.terminal.cursor_hidden()
            && self.terminal.scrollback() == 0;
        let (cursor_row, cursor_col) = self.terminal.cursor_position();
        let cursor_color = resolved_cursor_color(appearance, theme);
        let cursor_glyph_color = contrast_glyph_color(cursor_color, theme);
        let blink_hidden = cursor_visible
            && appearance.cursor_blink
            && !appearance
                .cursor_blink_speed
                .on_at(ui.input(|input| input.time));
        let search_current = self.search_current;

        for (row_index, line) in rows.iter().enumerate() {
            let search_matches = if search_query.is_empty() {
                Vec::new()
            } else {
                match_columns(line, &search_query)
            };

            for col in 0..self.last_grid.cols as usize {
                let Some(cell) = self.terminal.cell(row_index as u16, col as u16) else {
                    continue;
                };

                if cell.is_wide_continuation() {
                    continue;
                }

                let width = if cell.is_wide() { 2.0 } else { 1.0 };
                let cell_rect = egui::Rect::from_min_size(
                    top_left + egui::vec2(col as f32 * cell_width, row_index as f32 * cell_height),
                    egui::vec2(cell_width * width, cell_height),
                );

                let selected = selection.is_some_and(|(start, end)| {
                    row_index >= start.0
                        && row_index <= end.0
                        && col >= if row_index == start.0 { start.1 } else { 0 }
                        && col
                            < if row_index == end.0 {
                                end.1
                            } else {
                                self.last_grid.cols as usize
                            }
                });
                let searched = !selected
                    && search_len > 0
                    && search_matches
                        .iter()
                        .any(|start| col >= *start && col < *start + search_len);
                let current_match = !selected
                    && search_len > 0
                    && search_current.is_some_and(|(row, start)| {
                        row_index == row && col >= start && col < start + search_len
                    });

                let fg = if selected {
                    theme
                        .terminal
                        .selection_fg
                        .unwrap_or(theme.terminal.foreground)
                } else {
                    color_from_vt100(theme.terminal.foreground, cell.fgcolor(), theme)
                };
                let bg = if selected {
                    theme.terminal.selection_bg
                } else if current_match {
                    theme.terminal.search_current
                } else if searched {
                    theme.terminal.search_highlight
                } else {
                    color_from_vt100(theme.terminal.background, cell.bgcolor(), theme)
                };

                if bg != theme.terminal.background {
                    painter.rect_filled(cell_rect, 0.0, bg);
                }

                if cell.has_contents() {
                    painter.text(
                        cell_rect.left_top(),
                        egui::Align2::LEFT_TOP,
                        cell.contents(),
                        font_id.clone(),
                        fg,
                    );
                }
            }
        }

        if cursor_visible && !blink_hidden {
            let cursor_min = top_left
                + egui::vec2(
                    cursor_col as f32 * cell_width,
                    cursor_row as f32 * cell_height,
                );

            match appearance.cursor_style {
                CursorStyle::Block => {
                    let cell = self.terminal.cell(cursor_row, cursor_col);
                    let span = if cell.is_some_and(|cell| cell.is_wide()) {
                        2.0
                    } else {
                        1.0
                    };
                    let cursor_rect = egui::Rect::from_min_size(
                        cursor_min,
                        egui::vec2(cell_width * span, cell_height),
                    );
                    painter.rect_filled(cursor_rect, 2.0, cursor_color);
                    // Paint the covered glyph in a contrasting color so text
                    // stays readable under a filled block cursor.
                    if let Some(cell) = cell {
                        if cell.has_contents() {
                            painter.text(
                                cursor_rect.left_top(),
                                egui::Align2::LEFT_TOP,
                                cell.contents(),
                                font_id.clone(),
                                cursor_glyph_color,
                            );
                        }
                    }
                }
                CursorStyle::Beam => {
                    let thickness = appearance
                        .cursor_thickness
                        .clamp(1.0, (cell_width * 0.6).max(1.0));
                    let beam_rect = egui::Rect::from_min_size(
                        cursor_min + egui::vec2(1.0, 0.0),
                        egui::vec2(thickness, cell_height),
                    );
                    painter.rect_filled(beam_rect, 1.0, cursor_color);
                }
                CursorStyle::Underline => {
                    let thickness = appearance
                        .cursor_thickness
                        .clamp(1.0, (cell_height * 0.4).max(1.0));
                    let underline_rect = egui::Rect::from_min_size(
                        cursor_min + egui::vec2(0.0, cell_height - thickness - 1.0),
                        egui::vec2(cell_width, thickness),
                    );
                    painter.rect_filled(underline_rect, 1.0, cursor_color);
                }
            }
        }

        response
    }

    fn resize_if_needed(&mut self, available: egui::Vec2, typography: &TypographyConfig) {
        let cell_width = typography.cell_width().max(1.0);
        let cell_height = typography.cell_height().max(1.0);
        let cols = ((available.x - 20.0) / cell_width).floor().max(20.0) as u16;
        let rows = ((available.y - 16.0) / cell_height).floor().max(5.0) as u16;
        let next_grid = TerminalGrid { rows, cols };

        if next_grid == self.last_grid {
            return;
        }

        self.last_grid = next_grid;
        self.terminal.resize(next_grid);

        if let Ok(pty) = &self.pty {
            if let Err(error) = pty.resize(
                next_grid,
                cell_width.round() as u16,
                cell_height.round() as u16,
            ) {
                self.terminal
                    .process(format!("\r\n[ORBIT] resize failed: {error}\r\n").as_bytes());
            }
        }
    }

    fn adjust_scrollback(&mut self, delta: isize) {
        self.scrollback_rows = self
            .scrollback_rows
            .saturating_add_signed(delta)
            .min(MAX_SCROLLBACK_OFFSET);
        self.terminal.set_scrollback(self.scrollback_rows);
    }

    fn pointer_to_cell(
        &self,
        pos: egui::Pos2,
        rect: egui::Rect,
        cell_width: f32,
        cell_height: f32,
    ) -> (usize, usize) {
        let x = ((pos.x - rect.left() - 10.0) / cell_width).floor().max(0.0) as usize;
        let y = ((pos.y - rect.top() - 8.0) / cell_height).floor().max(0.0) as usize;
        (
            y.min(self.last_grid.rows.saturating_sub(1) as usize),
            x.min(self.last_grid.cols.saturating_sub(1) as usize),
        )
    }

    fn selected_text(&self) -> Option<String> {
        let selection = self.selection?;
        let (start, end) = selection.normalized();
        let rows = self.terminal.visible_rows();
        let mut output = String::new();
        for row in start.0..=end.0 {
            let line = rows.get(row)?;
            let chars: Vec<char> = line.chars().collect();
            let left = if row == start.0 { start.1 } else { 0 }.min(chars.len());
            let right = if row == end.0 { end.1 } else { chars.len() }.min(chars.len());
            if right > left {
                output.extend(chars[left..right].iter());
            }
            if row != end.0 {
                output.push('\n');
            }
        }
        (!output.is_empty()).then_some(output)
    }

    fn copy_selection(&mut self, ctx: &egui::Context) {
        if let Some(text) = self.selected_text() {
            ctx.copy_text(text);
            self.selection = None;
        }
    }

    fn write_to_pty(&mut self, bytes: &str) {
        if let Ok(pty) = &mut self.pty {
            self.scrollback_rows = 0;
            self.terminal.set_scrollback(0);
            if let Err(error) = pty.write_all(bytes.as_bytes()) {
                self.terminal
                    .process(format!("\r\n[ORBIT] write failed: {error}\r\n").as_bytes());
            }
        }
    }

    fn record_input_event(&mut self, event: &egui::Event) {
        match event {
            egui::Event::Text(text) | egui::Event::Paste(text) => {
                for character in text.chars() {
                    match character {
                        '\r' | '\n' => self.submit_current_command(),
                        character if !character.is_control() => {
                            self.current_command.push(character)
                        }
                        _ => {}
                    }
                }
            }
            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                ..
            } => self.submit_current_command(),
            egui::Event::Key {
                key: egui::Key::Backspace,
                pressed: true,
                ..
            } => {
                self.current_command.pop();
            }
            egui::Event::Key {
                key: egui::Key::U,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl => {
                self.current_command.clear();
            }
            _ => {}
        }
    }

    fn submit_current_command(&mut self) {
        let command = self.current_command.trim();
        if !command.is_empty() {
            self.history.push(CommandHistoryEntry {
                text: command.to_owned(),
                submitted_at: SystemTime::now(),
            });

            // Basic persistence: append to a local history file in the user's home directory.
            // Do not persist commands that look like passwords or secrets.
            if !command.to_lowercase().contains("password")
                && !command.to_lowercase().contains("secret")
                && !command.to_lowercase().contains("passwd")
            {
                if let Some(home) = std::env::var_os("HOME") {
                    let mut path = std::path::PathBuf::from(home);
                    path.push(".orbit_history");
                    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                        let _ = writeln!(file, "{}", command);
                    }
                }
            }
        }
        self.current_command.clear();
    }

    fn search_match_count(&self) -> usize {
        if self.search_query.is_empty() {
            return 0;
        }

        self.terminal
            .visible_rows()
            .iter()
            .map(|line| match_columns(line, &self.search_query).len())
            .sum()
    }

    fn find_all_matches(&self) -> Vec<(usize, usize)> {
        if self.search_query.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let rows = self.terminal.visible_rows();
        for (r, line) in rows.iter().enumerate() {
            for c in match_columns(line, &self.search_query) {
                out.push((r, c));
            }
        }
        out
    }

    fn find_next_match(&mut self) {
        let matches = self.find_all_matches();
        if std::env::var_os("ORBIT_DEBUG_EVENTS").is_some() {
            eprintln!(
                "[DBG] find_next: query={:?} matches={:?} current={:?}",
                self.search_query, matches, self.search_current
            );
        }
        if matches.is_empty() {
            return;
        }

        let (cur_r, cur_c) = if let Some(selection) = self.selection {
            selection.focus
        } else {
            let (r, c) = self.terminal.cursor_position();
            (r as usize, c as usize)
        };

        // Find first match after current position
        if let Some(&(mr, mc)) = matches
            .iter()
            .find(|&&(mr, mc)| mr > cur_r || (mr == cur_r && mc > cur_c))
        {
            self.selection = Some(Selection {
                anchor: (mr, mc),
                focus: (mr, mc + self.search_query.chars().count()),
            });
            self.search_current = Some((mr, mc));
            return;
        }

        // Wrap to first
        let (mr, mc) = matches[0];
        self.selection = Some(Selection {
            anchor: (mr, mc),
            focus: (mr, mc + self.search_query.chars().count()),
        });
        self.search_current = Some((mr, mc));
    }

    fn find_previous_match(&mut self) {
        let matches = self.find_all_matches();
        if matches.is_empty() {
            return;
        }

        let (cur_r, cur_c) = if let Some(selection) = self.selection {
            selection.focus
        } else {
            let (r, c) = self.terminal.cursor_position();
            (r as usize, c as usize)
        };

        // Find last match before current position
        if let Some(&(mr, mc)) = matches
            .iter()
            .rfind(|&&(mr, mc)| mr < cur_r || (mr == cur_r && mc < cur_c))
        {
            self.selection = Some(Selection {
                anchor: (mr, mc),
                focus: (mr, mc + self.search_query.chars().count()),
            });
            self.search_current = Some((mr, mc));
            return;
        }

        // Wrap to last
        let (mr, mc) = matches[matches.len() - 1];
        self.selection = Some(Selection {
            anchor: (mr, mc),
            focus: (mr, mc + self.search_query.chars().count()),
        });
        self.search_current = Some((mr, mc));
    }
}

impl Selection {
    fn normalized(self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

impl eframe::App for OrbitApp {
    /// Background the window is cleared with before panels are painted.
    ///
    /// With glass enabled the clear color is fully transparent so the desktop
    /// shows through the translucent material. With glass disabled it is the
    /// opaque theme background, so the window looks like a normal terminal
    /// even on surfaces that keep an alpha channel.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if self.glass.enabled {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            self.theme.ui.background.to_normalized_gamma_f32()
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if std::env::var_os("ORBIT_DEBUG_EVENTS").is_some() {
            let events = ctx.input(|input| input.events.clone());
            if !events.is_empty() {
                eprintln!("[DBG] {} events: {:?}", events.len(), events);
            }
            if !self.debug_focus_requested {
                if let Some(id) = self.debug_pane_id {
                    ctx.memory_mut(|memory| memory.request_focus(id));
                    self.debug_focus_requested = true;
                    eprintln!("[DBG] focusing terminal pane {:?}", id);
                }
            }
        }
        self.drain_pty();

        egui::TopBottomPanel::top("orbit_tabs")
            .frame(
                egui::Frame::new()
                    .fill(self.glass_panel())
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 4,
                        bottom: 4,
                    }),
            )
            .show(ctx, |ui| {
                self.ui_top_bar(ui);
                ui.add_space(2.0);
                self.ui_workspace_bar(ui);
                self.ui_search_bar(ui);
            });

        if self.theme_dirty || self.appearance_dirty {
            self.apply_theme(ctx);
            self.theme_dirty = false;
            self.appearance_dirty = false;
        }
        if self.typography_dirty {
            self.apply_typography(ctx);
            self.typography_dirty = false;
        }

        self.ui_history_panel(ctx);

        self.ui_workspace_dialog(ctx);
        self.ui_command_palette(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.glass_background()))
            .show(ctx, |ui| {
                let response = self.paint_active_tab(ui);
                self.handle_keyboard(ctx, response.has_focus());
            });

        self.update_glass_backdrop(ctx, frame);

        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn action_from_event(event: &egui::Event) -> Option<AppAction> {
    // If the OS/egui produced a Copy event (menu copy), respect it.
    if matches!(event, egui::Event::Copy) {
        return Some(AppAction::CopySelection);
    }

    let egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } = event
    else {
        return None;
    };

    // Non-Ctrl navigation keys
    if !modifiers.ctrl {
        // F3 -> Find next; Shift+F3 -> Find previous
        if matches!(key, egui::Key::F3) {
            return if modifiers.shift {
                Some(AppAction::FindPrevious)
            } else {
                Some(AppAction::FindNext)
            };
        }

        return match key {
            egui::Key::PageUp => Some(AppAction::PageUp),
            egui::Key::PageDown => Some(AppAction::PageDown),
            _ => None,
        };
    }

    // Ctrl+Alt+Arrow -> pane navigation
    if modifiers.alt && modifiers.ctrl {
        return match key {
            egui::Key::ArrowRight => Some(AppAction::NextPane),
            egui::Key::ArrowLeft => Some(AppAction::PreviousPane),
            _ => None,
        };
    }

    // Ctrl+Shift shortcuts
    if modifiers.shift {
        return match key {
            egui::Key::T => Some(AppAction::NewTab),
            egui::Key::W => Some(AppAction::CloseTab),
            egui::Key::F => Some(AppAction::ToggleSearch),
            egui::Key::H => Some(AppAction::ToggleHistory),
            egui::Key::D => Some(AppAction::SplitHorizontal),
            egui::Key::E => Some(AppAction::SplitVertical),
            egui::Key::R => Some(AppAction::RestartPane),
            egui::Key::X => Some(AppAction::ClosePane),
            egui::Key::Tab => Some(AppAction::PreviousTab),
            egui::Key::C => Some(AppAction::CopySelection), // Ctrl+Shift+C for copy
            egui::Key::P => Some(AppAction::ToggleCommandPalette),
            _ => None,
        };
    }

    // Plain Ctrl shortcuts should be forwarded to the terminal where appropriate.
    // We still capture Ctrl+Tab for tab switching and Ctrl+PageUp/PageDown for
    // workspace switching.
    match key {
        egui::Key::Tab => Some(AppAction::NextTab),
        egui::Key::PageDown => Some(AppAction::NextWorkspace),
        egui::Key::PageUp => Some(AppAction::PreviousWorkspace),
        _ => None,
    }
}

fn event_to_terminal_bytes(event: &egui::Event) -> Option<String> {
    match event {
        egui::Event::Text(text) => Some(text.clone()),
        egui::Event::Paste(text) => Some(text.clone()),
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            if modifiers.ctrl && !modifiers.shift {
                return ctrl_key_sequence(*key);
            }

            match key {
                egui::Key::Enter => Some("\r".to_owned()),
                egui::Key::Backspace => Some("\x7f".to_owned()),
                egui::Key::Tab => Some("\t".to_owned()),
                egui::Key::Escape => Some("\x1b".to_owned()),
                egui::Key::ArrowUp => Some("\x1b[A".to_owned()),
                egui::Key::ArrowDown => Some("\x1b[B".to_owned()),
                egui::Key::ArrowRight => Some("\x1b[C".to_owned()),
                egui::Key::ArrowLeft => Some("\x1b[D".to_owned()),
                egui::Key::Home => Some("\x1b[H".to_owned()),
                egui::Key::End => Some("\x1b[F".to_owned()),
                egui::Key::Delete => Some("\x1b[3~".to_owned()),
                egui::Key::PageUp => Some("\x1b[5~".to_owned()),
                egui::Key::PageDown => Some("\x1b[6~".to_owned()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn ctrl_key_sequence(key: egui::Key) -> Option<String> {
    let byte = match key {
        egui::Key::A => 0x01,
        egui::Key::B => 0x02,
        egui::Key::C => 0x03,
        egui::Key::D => 0x04,
        egui::Key::E => 0x05,
        egui::Key::F => 0x06,
        egui::Key::G => 0x07,
        egui::Key::H => 0x08,
        egui::Key::I => 0x09,
        egui::Key::J => 0x0a,
        egui::Key::K => 0x0b,
        egui::Key::L => 0x0c,
        egui::Key::M => 0x0d,
        egui::Key::N => 0x0e,
        egui::Key::O => 0x0f,
        egui::Key::P => 0x10,
        egui::Key::Q => 0x11,
        egui::Key::R => 0x12,
        egui::Key::S => 0x13,
        egui::Key::T => 0x14,
        egui::Key::U => 0x15,
        egui::Key::V => 0x16,
        egui::Key::W => 0x17,
        egui::Key::X => 0x18,
        egui::Key::Y => 0x19,
        egui::Key::Z => 0x1a,
        _ => return None,
    };

    Some((byte as u8 as char).to_string())
}

fn match_columns(line: &str, query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }

    let line_lower = line.to_lowercase();
    let query_lower = query.to_lowercase();
    line_lower
        .match_indices(&query_lower)
        .map(|(byte_index, _)| line[..byte_index].chars().count())
        .collect()
}

fn color_from_vt100(
    default_color: egui::Color32,
    color: vt100::Color,
    theme: &crate::theme::Theme,
) -> egui::Color32 {
    match color {
        vt100::Color::Default => default_color,
        vt100::Color::Idx(index) => theme.terminal.ansi[index as usize],
        vt100::Color::Rgb(red, green, blue) => egui::Color32::from_rgb(red, green, blue),
    }
}

/// Perceived brightness of a color on a 0..255 scale.
fn luminance(color: egui::Color32) -> f32 {
    color.r() as f32 * 0.299 + color.g() as f32 * 0.587 + color.b() as f32 * 0.114
}

/// Linear RGB interpolation between two colors.
fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    egui::Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t).round() as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t).round() as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t).round() as u8,
    )
}

/// The effective cursor color: theme cursor or custom color, always adjusted
/// so it stays clearly visible against the terminal background.
fn resolved_cursor_color(
    appearance: &AppearanceConfig,
    theme: &crate::theme::Theme,
) -> egui::Color32 {
    let color = match appearance.cursor_color_mode {
        CursorColorMode::Theme => theme.terminal.cursor,
        CursorColorMode::Custom => egui::Color32::from_rgb(
            appearance.cursor_custom_color[0],
            appearance.cursor_custom_color[1],
            appearance.cursor_custom_color[2],
        ),
    };
    ensure_cursor_contrast(color, theme)
}

/// Nudges a cursor color toward the theme foreground/background until it
/// differs enough from the terminal background to stay visible.
fn ensure_cursor_contrast(cursor: egui::Color32, theme: &crate::theme::Theme) -> egui::Color32 {
    const MIN_LUMINANCE_DIFF: f32 = 60.0;
    let background = theme.terminal.background;
    if (luminance(cursor) - luminance(background)).abs() >= MIN_LUMINANCE_DIFF {
        return cursor;
    }
    let to_fg = (luminance(cursor) - luminance(theme.terminal.foreground)).abs();
    let to_bg = (luminance(cursor) - luminance(background)).abs();
    let target = if to_fg > to_bg {
        theme.terminal.foreground
    } else {
        background
    };
    lerp_color(cursor, target, 0.7)
}

/// Glyph color to paint *on top of* a filled block cursor so the covered text
/// stays readable no matter which cursor color is configured.
fn contrast_glyph_color(cursor: egui::Color32, theme: &crate::theme::Theme) -> egui::Color32 {
    let to_fg = (luminance(cursor) - luminance(theme.terminal.foreground)).abs();
    let to_bg = (luminance(cursor) - luminance(theme.terminal.background)).abs();
    if to_fg > to_bg {
        theme.terminal.foreground
    } else {
        theme.terminal.background
    }
}

fn is_light_color(color: egui::Color32) -> bool {
    let red = color.r() as u32;
    let green = color.g() as u32;
    let blue = color.b() as u32;
    (red * 299 + green * 587 + blue * 114) >= 128_000
}

fn format_age(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else {
        format!("{}h ago", seconds / 3600)
    }
}

fn ui_input_escape(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.key_pressed(egui::Key::Escape))
}

fn item_action_label(action: &PaletteAction) -> &'static str {
    match action {
        PaletteAction::App(AppAction::NewTab) => "Open a new terminal tab",
        PaletteAction::App(AppAction::CloseTab) => "Close the active tab",
        PaletteAction::App(AppAction::NextTab) => "Go to the next tab",
        PaletteAction::App(AppAction::PreviousTab) => "Go to the previous tab",
        PaletteAction::App(AppAction::SplitHorizontal) => "Split the active pane horizontally",
        PaletteAction::App(AppAction::SplitVertical) => "Split the active pane vertically",
        PaletteAction::App(AppAction::ClosePane) => "Close the active pane",
        PaletteAction::App(AppAction::RestartPane) => "Restart the active pane's shell",
        PaletteAction::App(AppAction::ToggleSearch) => "Open or close the search bar",
        PaletteAction::App(AppAction::ToggleHistory) => "Open or close the command history panel",
        PaletteAction::App(_) => "",
        PaletteAction::SwitchToWorkspace(_) => "Switch to this workspace",
        PaletteAction::NewWorkspace => "Create a new workspace",
        PaletteAction::RenameWorkspace => "Rename the active workspace",
        PaletteAction::DuplicateWorkspace => "Duplicate the active workspace",
        PaletteAction::DeleteWorkspace => "Delete the active workspace",
        PaletteAction::SetDefaultWorkspace(_) => "Mark this workspace as default",
        PaletteAction::SaveWorkspace => "Save the current workspace state",
    }
}
