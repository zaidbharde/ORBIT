use crate::config::TerminalConfig;
use crate::pty::{PtyCommand, PtySession};
use crate::terminal::{TerminalGrid, TerminalState};
use eframe::egui;
use std::time::Duration;

const CELL_WIDTH: f32 = 8.5;
const CELL_HEIGHT: f32 = 18.0;

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
    terminal: TerminalState,
    pty: Result<PtySession, String>,
    last_grid: TerminalGrid,
    scrollback_rows: usize,
}

impl OrbitApp {
    fn new(_creation: &eframe::CreationContext<'_>) -> Self {
        let config = TerminalConfig::default();
        let last_grid = config.initial_grid;
        let terminal = TerminalState::new(last_grid, config.scrollback_lines);
        let pty = PtySession::spawn(config).map_err(|err| err.to_string());

        Self {
            terminal,
            pty,
            last_grid,
            scrollback_rows: 0,
        }
    }

    fn drain_pty(&mut self) {
        let Ok(pty) = &self.pty else {
            return;
        };

        while let Ok(command) = pty.output_rx().try_recv() {
            match command {
                PtyCommand::Output(bytes) => self.terminal.process(&bytes),
                PtyCommand::Exited(status) => {
                    let message = format!("\r\n[ORBIT] shell exited: {status}\r\n");
                    self.terminal.process(message.as_bytes());
                }
                PtyCommand::Error(error) => {
                    let message = format!("\r\n[ORBIT] PTY error: {error}\r\n");
                    self.terminal.process(message.as_bytes());
                }
            }
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context, has_focus: bool) {
        if !has_focus {
            return;
        }

        let events = ctx.input(|input| input.events.clone());
        for event in events {
            let Some(bytes) = event_to_terminal_bytes(&event) else {
                continue;
            };

            if let Ok(pty) = &mut self.pty {
                if let Err(error) = pty.write_all(bytes.as_bytes()) {
                    self.terminal
                        .process(format!("\r\n[ORBIT] write failed: {error}\r\n").as_bytes());
                }
            }
        }
    }

    fn resize_if_needed(&mut self, available: egui::Vec2) {
        let cols = (available.x / CELL_WIDTH).floor().max(20.0) as u16;
        let rows = (available.y / CELL_HEIGHT).floor().max(5.0) as u16;
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

    fn paint_terminal(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let available = ui.available_size();
        self.resize_if_needed(available);

        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click());
        if response.clicked() {
            response.request_focus();
        }

        let scroll_delta = ui.input(|input| input.smooth_scroll_delta.y + input.raw_scroll_delta.y);
        if response.hovered() && scroll_delta.abs() > 0.0 {
            self.scrollback_rows = self
                .scrollback_rows
                .saturating_add_signed((scroll_delta / CELL_HEIGHT).round() as isize)
                .min(10_000);
            self.terminal.set_scrollback(self.scrollback_rows);
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(10, 12, 14));

        let font_id = egui::FontId::monospace(14.0);
        let text_color = egui::Color32::from_rgb(220, 225, 230);
        let cursor_color = egui::Color32::from_rgb(120, 190, 255);
        let top_left = rect.left_top() + egui::vec2(10.0, 8.0);

        for (row_index, line) in self.terminal.visible_rows().into_iter().enumerate() {
            painter.text(
                top_left + egui::vec2(0.0, row_index as f32 * CELL_HEIGHT),
                egui::Align2::LEFT_TOP,
                line,
                font_id.clone(),
                text_color,
            );
        }

        let (cursor_row, cursor_col) = self.terminal.cursor_position();
        if response.has_focus() {
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
}

impl eframe::App for OrbitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_pty();

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let response = self.paint_terminal(ui);
                self.handle_keyboard(ctx, response.has_focus());
            });

        ctx.request_repaint_after(Duration::from_millis(16));
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
            if modifiers.ctrl {
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
