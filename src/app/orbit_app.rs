use crate::config::TerminalConfig;
use crate::pty::{PtyCommand, PtySession};
use crate::terminal::{TerminalGrid, TerminalState};
use eframe::egui;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{Duration, SystemTime};

const CELL_WIDTH: f32 = 8.5;
const CELL_HEIGHT: f32 = 18.0;
const MAX_SCROLLBACK_OFFSET: usize = 10_000;

pub fn run() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ORBIT")
            .with_app_id("dev.orbit.terminal")
            .with_inner_size([960.0, 640.0]),
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
    tabs: Vec<TerminalTab>,
    active_tab: usize,
    next_tab_id: usize,
    next_pane_id: usize,
    search_open: bool,
    history_open: bool,
    // Theme
    theme_name: String,
    theme: crate::theme::Theme,
    available_themes: Vec<&'static str>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitAxis {
    Horizontal,
    Vertical,
}

struct TerminalPane {
    id: usize,
    terminal: TerminalState,
    pty: Result<PtySession, String>,
    last_grid: TerminalGrid,
    scrollback_rows: usize,
    selection: Option<Selection>,
    search_query: String,
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
}

impl OrbitApp {
    fn new(_creation: &eframe::CreationContext<'_>) -> Self {
        let config = TerminalConfig::default();
        let mut next_pane_id = 1;
        let first_pane = TerminalPane::new(next_pane_id, &config, config.initial_grid);
        next_pane_id += 1;

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

        Self {
            config,
            tabs: vec![TerminalTab {
                id: 1,
                title: "shell 1".to_owned(),
                panes: PaneLayout::Single(first_pane),
                active_pane: 1,
            }],
            active_tab: 0,
            next_tab_id: 2,
            next_pane_id,
            search_open: false,
            history_open: false,
            theme_name,
            theme,
            available_themes,
        }
    }

    fn active_tab_mut(&mut self) -> Option<&mut TerminalTab> {
        self.tabs.get_mut(self.active_tab)
    }

    fn active_tab(&self) -> Option<&TerminalTab> {
        self.tabs.get(self.active_tab)
    }

    fn drain_pty(&mut self) {
        for tab in &mut self.tabs {
            tab.for_each_pane_mut(|pane| pane.drain_pty());
        }
    }

    fn ui_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for index in 0..self.tabs.len() {
                let selected = index == self.active_tab;
                let title = self.tabs[index].title.clone();
                if ui.selectable_label(selected, title).clicked() {
                    self.active_tab = index;
                }
            }

            ui.separator();

            if ui.button("+").on_hover_text("New tab").clicked() {
                self.new_tab();
            }
            if ui.button("x").on_hover_text("Close tab").clicked() {
                self.close_active_tab();
            }
            if ui.button("H").on_hover_text("Command history").clicked() {
                self.history_open = !self.history_open;
            }
            if ui.button("F").on_hover_text("Search").clicked() {
                self.search_open = !self.search_open;
            }

            ui.separator();
            // Theme selector
            egui::ComboBox::from_label("Theme")
                .selected_text(self.theme_name.clone())
                .show_ui(ui, |ui| {
                    for name in &self.available_themes {
                        if ui
                            .selectable_label(*name == self.theme_name, *name)
                            .clicked()
                        {
                            self.theme_name = name.to_string();
                            self.theme = crate::theme::get_theme(&self.theme_name);
                            // persist simple theme selection
                            if let Some(home) = std::env::var_os("HOME") {
                                let mut path = std::path::PathBuf::from(home);
                                path.push(".orbit_theme");
                                if let Ok(mut file) = std::fs::File::create(path) {
                                    let _ = writeln!(file, "{}", self.theme_name);
                                }
                            }
                        }
                    }
                });
        });
    }

    fn ui_search_bar(&mut self, ui: &mut egui::Ui) {
        if !self.search_open {
            return;
        }

        let active_pane_id = self.active_tab().map(|tab| tab.active_pane);
        let mut close_search = false;
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
                }
            }
            if let Some(id) = active_pane_id {
                ui.label(format!("pane {id}"));
            }
        });

        if close_search {
            self.search_open = false;
        }
    }

    fn ui_history_panel(&mut self, ctx: &egui::Context) {
        if !self.history_open {
            return;
        }

        egui::SidePanel::right("orbit_history")
            .resizable(true)
            .default_width(280.0)
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
        // Clone theme before mutably borrowing tabs to avoid borrow conflicts
        let theme = self.theme.clone();
        let Some(tab) = self.active_tab_mut() else {
            let (_, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
            return response;
        };

        tab.paint(ui, &theme)
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
            AppAction::ToggleSearch => self.search_open = !self.search_open,
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
        }
    }

    fn new_tab(&mut self) {
        let id = self.next_tab_id;
        self.next_tab_id += 1;

        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;

        let pane = TerminalPane::new(pane_id, &self.config, self.config.initial_grid);
        self.tabs.push(TerminalTab {
            id,
            title: format!("shell {id}"),
            panes: PaneLayout::Single(pane),
            active_pane: pane_id,
        });
        self.active_tab = self.tabs.len().saturating_sub(1);
    }

    fn close_active_tab(&mut self) {
        if self.tabs.len() <= 1 {
            self.restart_active_pane();
            return;
        }

        self.tabs.remove(self.active_tab);
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
    }

    fn select_next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = (self.active_tab + 1) % self.tabs.len();
    }

    fn select_previous_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active_tab = if self.active_tab == 0 {
            self.tabs.len() - 1
        } else {
            self.active_tab - 1
        };
    }

    fn split_active_pane(&mut self, axis: SplitAxis) {
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;

        let config = self.config.clone();
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        tab.split_active(axis, pane_id, &config);
    }

    fn close_active_pane(&mut self) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        tab.close_active_pane();
    }

    fn restart_active_pane(&mut self) {
        let config = self.config.clone();
        let Some(pane) = self.active_pane_mut() else {
            return;
        };
        pane.restart(&config);
    }

    fn active_pane_mut(&mut self) -> Option<&mut TerminalPane> {
        self.active_tab_mut()?.active_pane_mut()
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

    fn paint(&mut self, ui: &mut egui::Ui, theme: &crate::theme::Theme) -> egui::Response {
        match &mut self.panes {
            PaneLayout::Single(pane) => pane.paint(ui, self.active_pane == pane.id, theme),
            PaneLayout::Split {
                axis,
                first,
                second,
            } => match axis {
                SplitAxis::Horizontal => {
                    let available = ui.available_size();
                    let half_height = (available.y - 6.0).max(0.0) / 2.0;
                    let first_response = ui
                        .allocate_ui(egui::vec2(available.x, half_height), |ui| {
                            first.paint(ui, self.active_pane == first.id, theme)
                        })
                        .inner;
                    ui.add_space(6.0);
                    let second_response = ui
                        .allocate_ui(ui.available_size(), |ui| {
                            second.paint(ui, self.active_pane == second.id, theme)
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
                    let half_width = (available.x - 6.0).max(0.0) / 2.0;
                    let mut response = None;
                    ui.horizontal(|ui| {
                        let first_response = ui
                            .allocate_ui(egui::vec2(half_width, available.y), |ui| {
                                first.paint(ui, self.active_pane == first.id, theme)
                            })
                            .inner;
                        ui.add_space(6.0);
                        let second_response = ui
                            .allocate_ui(ui.available_size(), |ui| {
                                second.paint(ui, self.active_pane == second.id, theme)
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

    fn split_active(&mut self, axis: SplitAxis, pane_id: usize, config: &TerminalConfig) {
        let PaneLayout::Single(existing) = &self.panes else {
            self.active_pane = self.inactive_pane_id().unwrap_or(self.active_pane);
            return;
        };

        let grid = existing.last_grid;
        let new_pane = TerminalPane::new(pane_id, config, grid);
        let old = match std::mem::replace(
            &mut self.panes,
            PaneLayout::Single(TerminalPane::new(pane_id + 1, config, grid)),
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
    fn new(id: usize, config: &TerminalConfig, grid: TerminalGrid) -> Self {
        let mut pane_config = config.clone();
        pane_config.initial_grid = grid;
        let terminal = TerminalState::new(grid, pane_config.scrollback_lines);
        let pty = PtySession::spawn(pane_config).map_err(|err| err.to_string());

        Self {
            id,
            terminal,
            pty,
            last_grid: grid,
            scrollback_rows: 0,
            selection: None,
            search_query: String::new(),
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
            terminal: TerminalState::new(grid, 0),
            pty: Err("placeholder".to_owned()),
            last_grid: grid,
            scrollback_rows: 0,
            selection: None,
            search_query: String::new(),
            current_command: String::new(),
            history: Vec::new(),
            history_filter: String::new(),
            exited: true,
        }
    }

    fn restart(&mut self, config: &TerminalConfig) {
        let grid = self.last_grid;
        self.terminal = TerminalState::new(grid, config.scrollback_lines);
        self.pty = PtySession::spawn({
            let mut pane_config = config.clone();
            pane_config.initial_grid = grid;
            pane_config
        })
        .map_err(|err| err.to_string());
        self.scrollback_rows = 0;
        self.selection = None;
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
    ) -> egui::Response {
        let available = ui.available_size();
        self.resize_if_needed(available);

        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());
        if response.clicked() {
            response.request_focus();
        }

        let scroll_delta = ui.input(|input| input.smooth_scroll_delta.y + input.raw_scroll_delta.y);
        if response.hovered() && scroll_delta.abs() > 0.0 {
            self.adjust_scrollback((scroll_delta / CELL_HEIGHT).round() as isize);
        }

        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                let cell = self.pointer_to_cell(pos, rect);
                self.selection = Some(Selection {
                    anchor: cell,
                    focus: cell,
                });
            }
        } else if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let cell = self.pointer_to_cell(pos, rect);
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
        painter.rect_filled(rect, 0.0, theme.ui.panel);
        painter.rect_stroke(
            rect,
            0.0_f32,
            egui::Stroke::new(1.0_f32, border),
            egui::StrokeKind::Inside,
        );

        let font_id = egui::FontId::monospace(14.0);
        let text_color = theme.terminal.foreground;
        let cursor_color = theme.terminal.cursor;
        let top_left = rect.left_top() + egui::vec2(10.0, 8.0);
        let rows = self.terminal.visible_rows();

        for (row_index, line) in rows.iter().enumerate() {
            self.paint_selection(&painter, top_left, row_index, theme);
            self.paint_search_matches(&painter, top_left, row_index, line, theme);
            painter.text(
                top_left + egui::vec2(0.0, row_index as f32 * CELL_HEIGHT),
                egui::Align2::LEFT_TOP,
                line,
                font_id.clone(),
                text_color,
            );
        }

        let (cursor_row, cursor_col) = self.terminal.cursor_position();
        if response.has_focus() && is_active {
            let cursor_min = top_left
                + egui::vec2(
                    cursor_col as f32 * CELL_WIDTH,
                    cursor_row as f32 * CELL_HEIGHT,
                );
            let cursor_rect = egui::Rect::from_min_size(
                cursor_min,
                egui::vec2(CELL_WIDTH.max(1.0), CELL_HEIGHT.max(1.0)),
            );
            painter.rect_stroke(
                cursor_rect,
                0.0,
                egui::Stroke::new(1.0_f32, cursor_color),
                egui::StrokeKind::Inside,
            );
        }

        response
    }

    fn paint_selection(
        &self,
        painter: &egui::Painter,
        top_left: egui::Pos2,
        row_index: usize,
        theme: &crate::theme::Theme,
    ) {
        let Some(selection) = self.selection else {
            return;
        };

        let (start, end) = selection.normalized();
        if row_index < start.0 || row_index > end.0 {
            return;
        }

        let left = if row_index == start.0 { start.1 } else { 0 };
        let right = if row_index == end.0 {
            end.1
        } else {
            self.last_grid.cols as usize
        };

        if right <= left {
            return;
        }

        let highlight = egui::Rect::from_min_size(
            top_left + egui::vec2(left as f32 * CELL_WIDTH, row_index as f32 * CELL_HEIGHT),
            egui::vec2((right - left) as f32 * CELL_WIDTH, CELL_HEIGHT),
        );
        painter.rect_filled(highlight, 0.0, theme.terminal.selection_bg);
    }

    fn paint_search_matches(
        &self,
        painter: &egui::Painter,
        top_left: egui::Pos2,
        row_index: usize,
        line: &str,
        theme: &crate::theme::Theme,
    ) {
        if self.search_query.is_empty() {
            return;
        }

        for column in match_columns(line, &self.search_query) {
            let highlight = egui::Rect::from_min_size(
                top_left + egui::vec2(column as f32 * CELL_WIDTH, row_index as f32 * CELL_HEIGHT),
                egui::vec2(
                    self.search_query.chars().count() as f32 * CELL_WIDTH,
                    CELL_HEIGHT,
                ),
            );
            painter.rect_filled(highlight, 0.0, theme.terminal.search_highlight);
        }
    }

    fn resize_if_needed(&mut self, available: egui::Vec2) {
        let cols = ((available.x - 20.0) / CELL_WIDTH).floor().max(20.0) as u16;
        let rows = ((available.y - 16.0) / CELL_HEIGHT).floor().max(5.0) as u16;
        let next_grid = TerminalGrid { rows, cols };

        if next_grid == self.last_grid {
            return;
        }

        self.last_grid = next_grid;
        self.terminal.resize(next_grid);

        if let Ok(pty) = &self.pty {
            if let Err(error) = pty.resize(next_grid, CELL_WIDTH as u16, CELL_HEIGHT as u16) {
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

    fn pointer_to_cell(&self, pos: egui::Pos2, rect: egui::Rect) -> (usize, usize) {
        let x = ((pos.x - rect.left() - 10.0) / CELL_WIDTH).floor().max(0.0) as usize;
        let y = ((pos.y - rect.top() - 8.0) / CELL_HEIGHT).floor().max(0.0) as usize;
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
            return;
        }

        // Wrap to first
        let (mr, mc) = matches[0];
        self.selection = Some(Selection {
            anchor: (mr, mc),
            focus: (mr, mc + self.search_query.chars().count()),
        });
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
            return;
        }

        // Wrap to last
        let (mr, mc) = matches[matches.len() - 1];
        self.selection = Some(Selection {
            anchor: (mr, mc),
            focus: (mr, mc + self.search_query.chars().count()),
        });
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_pty();

        egui::TopBottomPanel::top("orbit_tabs").show(ctx, |ui| {
            self.ui_top_bar(ui);
            self.ui_search_bar(ui);
        });
        self.ui_history_panel(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let response = self.paint_active_tab(ui);
                self.handle_keyboard(ctx, response.has_focus());
            });

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
            _ => None,
        };
    }

    // Plain Ctrl shortcuts should be forwarded to the terminal where appropriate.
    // We still capture Ctrl+Tab for tab switching and Ctrl+Shift handled above.
    match key {
        egui::Key::Tab => Some(AppAction::NextTab),
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
