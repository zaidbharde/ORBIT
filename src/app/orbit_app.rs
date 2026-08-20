use crate::color::is_light_color;
use crate::config::{
    CursorColorMode, CursorStyle, GlassConfig, GlassTint, TerminalConfig, TypographyConfig,
};
use crate::glass::GlassBackend;
use crate::glass::backend::{apply_x11_blur_region, x11_window_id};
use crate::section::registry::SectionRegistry;
use crate::section::{SectionAction, SectionContext, SectionId};
use crate::theme::Theme;
use eframe::egui;
use raw_window_handle::HasWindowHandle;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

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

/// Actions handled by the global ORBIT chrome (not by sections). Section
/// actions are forwarded to the active section, which decides whether it
/// supports them.
#[derive(Clone, Copy, Debug)]
enum GlobalAction {
    SwitchToSection(SectionId),
    ToggleCommandPalette,
    Section(SectionAction),
}

struct OrbitApp {
    config: TerminalConfig,
    sections: SectionRegistry,
    // Command palette
    command_palette_open: bool,
    palette_filter: String,
    palette_selected: usize,
    palette_just_opened: bool,
    // Theme
    theme_name: String,
    theme: Theme,
    available_themes: Vec<&'static str>,
    // Glass / acrylic material
    glass: GlassConfig,
    glass_settings_open: bool,
    appearance_settings_open: bool,
    typography_dirty: bool,
    theme_dirty: bool,
    appearance_dirty: bool,
    backend: &'static GlassBackend,
    last_blur_size: Option<egui::Vec2>,
    debug_pane_id: Option<egui::Id>,
    debug_focus_requested: bool,
}

impl OrbitApp {
    fn new(creation: &eframe::CreationContext<'_>) -> Self {
        let config = TerminalConfig::load();
        let sections = SectionRegistry::new(&config);

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
            sections,
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
            typography_dirty: true,
            theme_dirty: true,
            appearance_dirty: false,
            backend: crate::glass::backend::probe(),
            last_blur_size: None,
            debug_pane_id: None,
            debug_focus_requested: false,
        };
        eprintln!("[ORBIT] glass backend: {}", app.backend.describe());
        app.apply_typography(&creation.egui_ctx);
        app.apply_theme(&creation.egui_ctx);
        app.typography_dirty = false;
        app.theme_dirty = false;
        app
    }

    /// Switches the active section, restores its focus and persists the
    /// choice. Never restarts ORBIT, the shell or any section state.
    fn switch_section(&mut self, id: SectionId, ctx: &egui::Context) {
        if self.sections.active_id() == id {
            return;
        }
        if self.sections.switch_to(id) {
            self.sections.active_mut().on_activated(ctx);
            self.persist_config();
        }
    }

    fn ui_global_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
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
                    let (status_text, status_color) =
                        match self.sections.active().status_label(&self.theme) {
                            Some((text, color)) => (text, color),
                            None => (String::new(), self.theme.ui.secondary_text),
                        };
                    if !status_text.is_empty() {
                        ui.colored_label(status_color, status_text);
                    }
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
    }

    /// Left navigation rail: one chip per registered section plus the
    /// active section status line at the bottom.
    fn ui_section_rail(&mut self, ctx: &egui::Context) {
        let scale = self.config.appearance.spacing_scale.clamp(0.8, 1.4);
        let frame = egui::Frame::new()
            .fill(self.glass_panel())
            .inner_margin(egui::Margin::same(8));
        egui::SidePanel::left("orbit_section_rail")
            .resizable(false)
            .exact_width(172.0 * scale)
            .frame(frame)
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new("ORBIT")
                        .strong()
                        .size(17.0)
                        .color(self.theme.ui.text),
                );
                ui.label(
                    egui::RichText::new("sections")
                        .small()
                        .color(self.theme.ui.secondary_text),
                );
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);

                let mut switch: Option<SectionId> = None;
                for index in 0..self.sections.len() {
                    if self.ui_section_chip(ui, index) {
                        switch = self
                            .sections
                            .section(index)
                            .map(|section| section.id())
                            .or(switch);
                    }
                }

                ui.add_space(2.0);
                ui.separator();
                let active_name = self.sections.active_id().name();
                ui.label(
                    egui::RichText::new(format!("{active_name} active"))
                        .small()
                        .color(self.theme.ui.secondary_text),
                );
                ui.label(
                    egui::RichText::new("Ctrl+Shift+P: palette")
                        .small()
                        .color(self.theme.ui.secondary_text),
                );

                if let Some(id) = switch {
                    self.switch_section(id, ctx);
                }
            });
    }

    /// One section chip in the navigation rail. Returns `true` when clicked.
    fn ui_section_chip(&mut self, ui: &mut egui::Ui, index: usize) -> bool {
        let Some(id) = self.sections.section(index).map(|section| section.id()) else {
            return false;
        };
        let selected = index == self.sections.active_index();
        let descriptor = id.descriptor();
        let theme = self.theme.clone();
        let appearance = self.config.appearance.clone();
        let radius = appearance.panel_radius.clamp(0.0, 12.0) as u8;

        let width = ui.available_width();
        let height = 30.0 * appearance.spacing_scale.clamp(0.8, 1.4);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

        let hovered = response.hovered();
        let fill = if selected {
            theme.ui.tab_active
        } else if hovered {
            crate::color::lerp_color(theme.ui.tab_inactive, theme.ui.tab_active, 0.45)
        } else {
            theme.ui.tab_inactive
        };
        let painter = ui.painter();
        painter.rect_filled(rect, radius, fill);
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0_f32, theme.ui.divider),
            egui::StrokeKind::Inside,
        );
        if selected {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    rect.left_top(),
                    egui::pos2(rect.left() + 3.0, rect.bottom()),
                ),
                1.5,
                theme.ui.accent,
            );
        }

        let label = format!("{} {}", descriptor.icon, descriptor.name);
        let font_id = egui::TextStyle::Body.resolve(ui.style());
        let text_size = ui
            .fonts(|fonts| fonts.layout_no_wrap(label.clone(), font_id.clone(), theme.ui.text))
            .size();
        painter.text(
            rect.left_top() + egui::vec2(10.0, (height - text_size.y) / 2.0),
            egui::Align2::LEFT_TOP,
            label,
            font_id,
            if selected {
                theme.ui.text
            } else {
                theme.ui.secondary_text
            },
        );
        if selected || hovered {
            painter.text(
                rect.right_center() - egui::vec2(8.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                descriptor.shortcut,
                egui::FontId::proportional(10.0),
                theme.ui.secondary_text,
            );
        }

        response
            .on_hover_text(format!(
                "{}\n{}",
                descriptor.description, descriptor.shortcut
            ))
            .clicked()
    }

    fn palette_items(&self) -> Vec<(String, GlobalAction)> {
        let mut items = Vec::new();
        for id in SectionId::ALL {
            let descriptor = id.descriptor();
            items.push((
                format!("Section: {}", descriptor.name),
                GlobalAction::SwitchToSection(id),
            ));
        }
        let terminal =
            |label: &str, action: SectionAction| (label.to_owned(), GlobalAction::Section(action));
        items.push(terminal("Terminal: New tab", SectionAction::NewTab));
        items.push(terminal("Terminal: Close tab", SectionAction::CloseTab));
        items.push(terminal("Terminal: Next tab", SectionAction::NextTab));
        items.push(terminal(
            "Terminal: Previous tab",
            SectionAction::PreviousTab,
        ));
        items.push(terminal(
            "Terminal: Split horizontal",
            SectionAction::SplitHorizontal,
        ));
        items.push(terminal(
            "Terminal: Split vertical",
            SectionAction::SplitVertical,
        ));
        items.push(terminal("Terminal: Close pane", SectionAction::ClosePane));
        items.push(terminal(
            "Terminal: Restart pane",
            SectionAction::RestartPane,
        ));
        items.push(terminal(
            "Terminal: Toggle search",
            SectionAction::ToggleSearch,
        ));
        items.push(terminal(
            "Terminal: Toggle history",
            SectionAction::ToggleHistory,
        ));
        items
    }

    fn ui_command_palette(&mut self, ctx: &egui::Context) {
        if !self.command_palette_open {
            return;
        }
        let items = self.palette_items();
        let mut open = true;
        let mut action: Option<GlobalAction> = None;
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
                    action = Some(items[filtered[self.palette_selected]].1);
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
                                action = Some(*item_action);
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

    fn run_palette_action(&mut self, action: GlobalAction, ctx: &egui::Context) {
        match action {
            GlobalAction::SwitchToSection(id) => self.switch_section(id, ctx),
            GlobalAction::Section(action) => self.sections.active_mut().action(action, ctx),
            GlobalAction::ToggleCommandPalette => {}
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
        self.config.active_section = self.sections.active_id().config_id().to_owned();
        let _ = self.config.save();
    }

    fn run_action(&mut self, action: GlobalAction, ctx: &egui::Context) {
        match action {
            GlobalAction::SwitchToSection(id) => self.switch_section(id, ctx),
            GlobalAction::ToggleCommandPalette => {
                self.command_palette_open = !self.command_palette_open;
                self.palette_just_opened = self.command_palette_open;
                self.palette_selected = 0;
                self.palette_filter.clear();
            }
            GlobalAction::Section(action) => self.sections.active_mut().action(action, ctx),
        }
    }

    /// Global shortcuts first; the remaining events are forwarded to the
    /// active section, which decides what to do with them.
    fn handle_keyboard(&mut self, ctx: &egui::Context, focused: bool) {
        let events = ctx.input(|input| input.events.clone());

        for event in events {
            if let Some(action) = action_from_event(&event) {
                self.run_action(action, ctx);
                continue;
            }

            self.sections
                .active_mut()
                .handle_keyboard(ctx, &event, focused);
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

        // Only the active section advances (PTY draining etc.); inactive
        // sections hold their state untouched.
        self.sections.active_mut().update(ctx);

        egui::TopBottomPanel::top("orbit_global_bar")
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
                self.ui_global_bar(ui);
                ui.add_space(2.0);
                let context = SectionContext {
                    theme: &self.theme,
                    typography: &self.config.typography,
                    glass: &self.glass,
                    appearance: &self.config.appearance,
                    panel_fill: self.glass_panel(),
                };
                self.sections.active_mut().top_bar(ui, &context);
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

        self.ui_section_rail(ctx);

        {
            let context = SectionContext {
                theme: &self.theme,
                typography: &self.config.typography,
                glass: &self.glass,
                appearance: &self.config.appearance,
                panel_fill: self.glass_panel(),
            };
            self.sections.active_mut().overlays(ctx, &context);
        }

        self.ui_command_palette(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.glass_background()))
            .show(ctx, |ui| {
                let context = SectionContext {
                    theme: &self.theme,
                    typography: &self.config.typography,
                    glass: &self.glass,
                    appearance: &self.config.appearance,
                    panel_fill: self.glass_panel(),
                };
                let response = self.sections.active_mut().render(ui, &context);
                if std::env::var_os("ORBIT_DEBUG_EVENTS").is_some() {
                    self.debug_pane_id = Some(response.id);
                }
                self.handle_keyboard(ctx, response.has_focus());
            });

        self.update_glass_backdrop(ctx, frame);

        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn action_from_event(event: &egui::Event) -> Option<GlobalAction> {
    // If the OS/egui produced a Copy event (menu copy), respect it.
    if matches!(event, egui::Event::Copy) {
        return Some(GlobalAction::Section(SectionAction::CopySelection));
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
                Some(GlobalAction::Section(SectionAction::FindPrevious))
            } else {
                Some(GlobalAction::Section(SectionAction::FindNext))
            };
        }

        return match key {
            egui::Key::PageUp => Some(GlobalAction::Section(SectionAction::PageUp)),
            egui::Key::PageDown => Some(GlobalAction::Section(SectionAction::PageDown)),
            _ => None,
        };
    }

    // Ctrl+Alt+Arrow -> pane navigation
    if modifiers.alt {
        return match key {
            egui::Key::ArrowRight => Some(GlobalAction::Section(SectionAction::NextPane)),
            egui::Key::ArrowLeft => Some(GlobalAction::Section(SectionAction::PreviousPane)),
            _ => None,
        };
    }

    // Ctrl+Shift shortcuts
    if modifiers.shift {
        return match key {
            egui::Key::T => Some(GlobalAction::Section(SectionAction::NewTab)),
            egui::Key::W => Some(GlobalAction::Section(SectionAction::CloseTab)),
            egui::Key::F => Some(GlobalAction::Section(SectionAction::ToggleSearch)),
            egui::Key::H => Some(GlobalAction::Section(SectionAction::ToggleHistory)),
            egui::Key::D => Some(GlobalAction::Section(SectionAction::SplitHorizontal)),
            egui::Key::E => Some(GlobalAction::Section(SectionAction::SplitVertical)),
            egui::Key::R => Some(GlobalAction::Section(SectionAction::RestartPane)),
            egui::Key::X => Some(GlobalAction::Section(SectionAction::ClosePane)),
            egui::Key::Tab => Some(GlobalAction::Section(SectionAction::PreviousTab)),
            egui::Key::C => Some(GlobalAction::Section(SectionAction::CopySelection)), // Ctrl+Shift+C for copy
            egui::Key::P => Some(GlobalAction::ToggleCommandPalette),
            _ => None,
        };
    }

    // Plain Ctrl shortcuts. Ctrl+number has no control byte, so it was never
    // forwarded to the shell — using it for section switching cannot conflict
    // with terminal behavior.
    match key {
        egui::Key::Tab => Some(GlobalAction::Section(SectionAction::NextTab)),
        egui::Key::Num1 => Some(GlobalAction::SwitchToSection(SectionId::Terminal)),
        egui::Key::Num2 => Some(GlobalAction::SwitchToSection(SectionId::Coding)),
        egui::Key::Num3 => Some(GlobalAction::SwitchToSection(SectionId::Networking)),
        egui::Key::Num4 => Some(GlobalAction::SwitchToSection(SectionId::Cybersecurity)),
        egui::Key::Num5 => Some(GlobalAction::SwitchToSection(SectionId::DevOps)),
        egui::Key::Num6 => Some(GlobalAction::SwitchToSection(SectionId::System)),
        _ => None,
    }
}

fn ui_input_escape(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.key_pressed(egui::Key::Escape))
}

fn item_action_label(action: &GlobalAction) -> &'static str {
    match action {
        GlobalAction::SwitchToSection(id) => match id {
            SectionId::Terminal => "Switch to the Terminal section",
            SectionId::Coding => "Switch to the Coding section",
            SectionId::Networking => "Switch to the Networking section",
            SectionId::Cybersecurity => "Switch to the Cybersecurity section",
            SectionId::DevOps => "Switch to the DevOps section",
            SectionId::System => "Switch to the System section",
        },
        GlobalAction::ToggleCommandPalette => "Toggle the command palette",
        GlobalAction::Section(action) => match action {
            SectionAction::NewTab => "Open a new terminal tab",
            SectionAction::CloseTab => "Close the active tab",
            SectionAction::NextTab => "Go to the next tab",
            SectionAction::PreviousTab => "Go to the previous tab",
            SectionAction::SplitHorizontal => "Split the active pane horizontally",
            SectionAction::SplitVertical => "Split the active pane vertically",
            SectionAction::ClosePane => "Close the active pane",
            SectionAction::RestartPane => "Restart the active pane's shell",
            SectionAction::ToggleSearch => "Open or close the search bar",
            SectionAction::ToggleHistory => "Open or close the command history panel",
            SectionAction::CopySelection => "Copy the terminal selection",
            SectionAction::FindNext => "Jump to the next search match",
            SectionAction::FindPrevious => "Jump to the previous search match",
            SectionAction::NextPane => "Focus the next pane",
            SectionAction::PreviousPane => "Focus the previous pane",
            SectionAction::PageUp => "Scroll the terminal up",
            SectionAction::PageDown => "Scroll the terminal down",
        },
    }
}
