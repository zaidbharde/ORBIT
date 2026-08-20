use super::{Section, SectionAction, SectionContext, SectionId};
use crate::color::{lerp_color, luminance};
use crate::config::{AppearanceConfig, GlassConfig, TerminalConfig, TypographyConfig};
use crate::glass::{glass_fill, with_alpha};
use crate::pty::{PtyCommand, PtySession};
use crate::terminal::{TerminalGrid, TerminalState};
use eframe::egui;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const MAX_SCROLLBACK_OFFSET: usize = 10_000;

/// The ORBIT terminal: PTY-backed shell with tabs, panes, splits, search
/// and command history. This is the primary section and the only one with
/// real tools in the current phase.
///
/// All terminal state lives here and survives section switches — switching
/// away never restarts the shell or resets the screen.
pub struct TerminalSection {
    config: TerminalConfig,
    tabs: Vec<TerminalTab>,
    active_tab: usize,
    next_tab_id: usize,
    next_pane_id: usize,
    search_open: bool,
    history_open: bool,
    /// The egui id of the terminal surface, restored on reactivation.
    focus_id: Option<egui::Id>,
    focus_requested: bool,
}

impl TerminalSection {
    pub fn new(config: &TerminalConfig) -> Self {
        let working_dir = if config.working_dir.is_dir() {
            config.working_dir.clone()
        } else {
            home_dir()
        };
        let pane = TerminalPane::new(
            1,
            config,
            config.initial_grid,
            working_dir,
            "shell 1".to_owned(),
        );
        let tab = TerminalTab {
            title: "shell 1".to_owned(),
            panes: PaneLayout::Single(pane),
            active_pane: 1,
        };
        Self {
            config: config.clone(),
            tabs: vec![tab],
            active_tab: 0,
            next_tab_id: 2,
            next_pane_id: 2,
            search_open: false,
            history_open: false,
            focus_id: None,
            focus_requested: true,
        }
    }

    fn active_tab_mut(&mut self) -> Option<&mut TerminalTab> {
        self.tabs.get_mut(self.active_tab)
    }

    fn active_tab(&self) -> Option<&TerminalTab> {
        self.tabs.get(self.active_tab)
    }

    fn active_pane_mut(&mut self) -> Option<&mut TerminalPane> {
        let tab = self.tabs.get_mut(self.active_tab)?;
        tab.active_pane_mut()
    }

    fn new_tab(&mut self) {
        let config = self.config.clone();
        let (tab_id, pane_id, dir) = {
            let dir = if self.config.working_dir.is_dir() {
                self.config.working_dir.clone()
            } else {
                home_dir()
            };
            let tab_id = self.next_tab_id;
            let pane_id = self.next_pane_id;
            self.next_tab_id += 1;
            self.next_pane_id += 1;
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
            title: format!("shell {tab_id}"),
            panes: PaneLayout::Single(pane),
            active_pane: pane_id,
        };
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    fn close_active_tab(&mut self) {
        if self.tabs.len() <= 1 {
            self.restart_active_pane();
            return;
        }
        self.tabs.remove(self.active_tab);
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
    }

    /// Closes the tab at `index` (used by the per-tab close button). Closing
    /// the last tab restarts its pane instead, like `close_active_tab`.
    fn close_tab(&mut self, index: usize) {
        if self.tabs.len() <= 1 {
            self.restart_active_pane();
            return;
        }
        if index < self.tabs.len() {
            self.tabs.remove(index);
            if index < self.active_tab {
                self.active_tab -= 1;
            }
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        }
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
        let config = self.config.clone();
        let (pane_id, dir) = {
            let dir = self
                .tabs
                .get(self.active_tab)
                .and_then(|tab| tab.active_pane())
                .map(|pane| pane.working_dir.clone())
                .unwrap_or_else(|| home_dir());
            let pane_id = self.next_pane_id;
            self.next_pane_id += 1;
            (pane_id, dir)
        };
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        tab.split_active(axis, pane_id, &config, dir);
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

    /// One tab chip: rounded background, hover highlight, accent underline on
    /// the active tab and a per-tab close button. Returns `true` when this
    /// tab's close button was clicked.
    fn ui_tab(&mut self, ui: &mut egui::Ui, index: usize, context: &SectionContext<'_>) -> bool {
        let selected = self.active_tab == index;
        let Some(title) = self.tabs.get(index).map(|tab| tab.title.clone()) else {
            return false;
        };
        let theme = context.theme.clone();
        let appearance = context.appearance.clone();
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
            self.active_tab = index;
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

    fn ui_search_bar(&mut self, ui: &mut egui::Ui, context: &SectionContext<'_>) {
        if !self.search_open {
            return;
        }
        let _ = context;

        let active_pane_id = self.active_tab().map(|tab| tab.active_pane);
        let mut close_search = false;
        ui.horizontal(|ui| {
            ui.label("Search");
            if let Some(pane) = self.active_pane_mut() {
                // Avoid borrowing pane across UI input checks that mutate
                // self; use a local copy and assign back.
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
    }

    fn ui_history_panel(&mut self, ctx: &egui::Context, context: &SectionContext<'_>) {
        if !self.history_open {
            return;
        }

        let border_color = with_alpha(context.theme.ui.border, context.appearance.border_opacity);
        let frame = egui::Frame::new()
            .fill(context.panel_fill)
            .corner_radius(context.appearance.panel_radius.clamp(0.0, 16.0) as u8)
            .inner_margin(egui::Margin::same(10))
            .stroke(egui::Stroke::new(
                context.appearance.border_width.clamp(0.0, 4.0),
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
}

impl Section for TerminalSection {
    fn id(&self) -> SectionId {
        SectionId::Terminal
    }

    fn update(&mut self, _ctx: &egui::Context) {
        for tab in &mut self.tabs {
            tab.for_each_pane_mut(|pane| pane.drain_pty());
        }
    }

    fn render(&mut self, ui: &mut egui::Ui, context: &SectionContext<'_>) -> egui::Response {
        let theme = context.theme.clone();
        let typography = context.typography.clone();
        let glass = context.glass.clone();
        let appearance = context.appearance.clone();
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            let (_, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
            return response;
        };

        let response = tab.paint(ui, &theme, &typography, &glass, &appearance);
        self.focus_id = Some(response.id);
        if self.focus_requested {
            response.request_focus();
            self.focus_requested = false;
        }
        response
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, context: &SectionContext<'_>) {
        let mut close_tab: Option<usize> = None;
        let mut new_tab_requested = false;
        let mut toggle_search = false;
        let mut toggle_history = false;
        ui.horizontal(|ui| {
            for index in 0..self.tabs.len() {
                if self.ui_tab(ui, index, context) {
                    close_tab = Some(index);
                }
            }

            ui.separator();

            if ui
                .button("+")
                .on_hover_text("New tab (Ctrl+Shift+T)")
                .clicked()
            {
                new_tab_requested = true;
            }
            if ui
                .button("H")
                .on_hover_text("Command history (Ctrl+Shift+H)")
                .clicked()
            {
                toggle_history = true;
            }
            if ui
                .button("F")
                .on_hover_text("Search (Ctrl+Shift+F)")
                .clicked()
            {
                toggle_search = true;
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

        ui.add_space(2.0);
        self.ui_search_bar(ui, context);
    }

    fn overlays(&mut self, ctx: &egui::Context, context: &SectionContext<'_>) {
        self.ui_history_panel(ctx, context);
    }

    fn handle_keyboard(&mut self, _ctx: &egui::Context, event: &egui::Event, focused: bool) {
        if !focused {
            return;
        }

        if self.search_open {
            if matches!(
                event,
                egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Key { .. }
            ) {
                return;
            }
        }

        let Some(pane) = self.active_pane_mut() else {
            return;
        };

        let Some(bytes) = event_to_terminal_bytes(event) else {
            return;
        };

        pane.record_input_event(event);
        pane.write_to_pty(&bytes);
    }

    fn action(&mut self, action: SectionAction, ctx: &egui::Context) {
        match action {
            SectionAction::NewTab => self.new_tab(),
            SectionAction::CloseTab => self.close_active_tab(),
            SectionAction::NextTab => self.select_next_tab(),
            SectionAction::PreviousTab => self.select_previous_tab(),
            SectionAction::SplitHorizontal => self.split_active_pane(SplitAxis::Horizontal),
            SectionAction::SplitVertical => self.split_active_pane(SplitAxis::Vertical),
            SectionAction::ClosePane => self.close_active_pane(),
            SectionAction::RestartPane => self.restart_active_pane(),
            SectionAction::ToggleSearch => {
                self.search_open = !self.search_open;
            }
            SectionAction::ToggleHistory => self.history_open = !self.history_open,
            SectionAction::CopySelection => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.copy_selection(ctx);
                }
            }
            SectionAction::FindNext => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.find_next_match();
                }
            }
            SectionAction::FindPrevious => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.find_previous_match();
                }
            }
            SectionAction::NextPane => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.focus_next_pane();
                }
            }
            SectionAction::PreviousPane => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.focus_previous_pane();
                }
            }
            SectionAction::PageUp => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.adjust_scrollback(20);
                }
            }
            SectionAction::PageDown => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.adjust_scrollback(-20);
                }
            }
        }
    }

    fn status_label(&self, theme: &crate::theme::Theme) -> Option<(String, egui::Color32)> {
        match self.active_tab() {
            Some(tab) => match tab.active_pane() {
                Some(pane) if pane.exited => Some(("shell exited".to_owned(), theme.status.error)),
                Some(pane) if pane.pty.is_err() => {
                    Some(("pty error".to_owned(), theme.status.error))
                }
                Some(_) => Some(("running".to_owned(), theme.status.success)),
                None => Some(("no active pane".to_owned(), theme.status.warning)),
            },
            None => Some(("no tabs".to_owned(), theme.status.warning)),
        }
    }

    fn on_activated(&mut self, _ctx: &egui::Context) {
        self.focus_requested = true;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum SplitAxis {
    Horizontal,
    Vertical,
}

struct TerminalTab {
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
            glass_fill(
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
        let border_color = with_alpha(border, appearance.border_opacity);
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
                crate::config::CursorStyle::Block => {
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
                crate::config::CursorStyle::Beam => {
                    let thickness = appearance
                        .cursor_thickness
                        .clamp(1.0, (cell_width * 0.6).max(1.0));
                    let beam_rect = egui::Rect::from_min_size(
                        cursor_min + egui::vec2(1.0, 0.0),
                        egui::vec2(thickness, cell_height),
                    );
                    painter.rect_filled(beam_rect, 1.0, cursor_color);
                }
                crate::config::CursorStyle::Underline => {
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

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
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

/// The effective cursor color: theme cursor or custom color, always adjusted
/// so it stays clearly visible against the terminal background.
fn resolved_cursor_color(
    appearance: &AppearanceConfig,
    theme: &crate::theme::Theme,
) -> egui::Color32 {
    let color = match appearance.cursor_color_mode {
        crate::config::CursorColorMode::Theme => theme.terminal.cursor,
        crate::config::CursorColorMode::Custom => egui::Color32::from_rgb(
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
