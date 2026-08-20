use super::{Section, SectionContext, SectionId};
use crate::glass::with_alpha;
use eframe::egui;

/// A section whose tools arrive in a future ORBIT phase.
///
/// Renders a clean, themed placeholder instead of an error screen. It holds
/// no state and does no work; it exists so every built-in section has a
/// visible home from day one.
pub struct PlaceholderSection {
    id: SectionId,
}

impl PlaceholderSection {
    pub fn new(id: SectionId) -> Self {
        Self { id }
    }
}

impl Section for PlaceholderSection {
    fn id(&self) -> SectionId {
        self.id
    }

    fn render(&mut self, ui: &mut egui::Ui, context: &SectionContext<'_>) -> egui::Response {
        let descriptor = self.id.descriptor();
        let theme = context.theme;
        let appearance = context.appearance;

        let available = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::hover());
        let painter = ui.painter_at(rect);

        let radius = appearance.panel_radius.clamp(0.0, 16.0);
        painter.rect_filled(rect, radius, context.panel_fill);
        if appearance.border_width > 0.0 {
            painter.rect_stroke(
                rect,
                radius,
                egui::Stroke::new(
                    appearance.border_width.clamp(0.0, 4.0),
                    with_alpha(theme.ui.border, appearance.border_opacity),
                ),
                egui::StrokeKind::Inside,
            );
        }

        let icon_font = egui::FontId::proportional(30.0);
        let title_font = egui::FontId::proportional(19.0);
        let body_font = egui::FontId::proportional(13.0);
        let note_font = egui::FontId::proportional(12.0);

        let measure = |text: &str, font: &egui::FontId| -> egui::Vec2 {
            ui.fonts(|fonts| fonts.layout_no_wrap(text.to_owned(), font.clone(), theme.ui.text))
                .size()
        };

        let icon = descriptor.icon;
        let title = descriptor.name;
        let description = descriptor.description;
        let note = "Tools will be available in a future ORBIT phase.";

        let icon_size = measure(icon, &icon_font);
        let title_size = measure(title, &title_font);
        let description_size = measure(description, &body_font);
        let note_size = measure(note, &note_font);

        let gap = 10.0;
        let block_height =
            icon_size.y + gap + title_size.y + gap + description_size.y + gap + note_size.y;
        let center_x = rect.center().x;
        let mut y = rect.center().y - block_height / 2.0;

        let center_line = |painter: &egui::Painter,
                           y: f32,
                           size: egui::Vec2,
                           text: &str,
                           font: egui::FontId,
                           color: egui::Color32| {
            painter.text(
                egui::pos2(center_x, y + size.y / 2.0),
                egui::Align2::CENTER_CENTER,
                text,
                font,
                color,
            );
        };

        center_line(&painter, y, icon_size, icon, icon_font, theme.ui.accent);
        y += icon_size.y + gap;
        center_line(&painter, y, title_size, title, title_font, theme.ui.text);
        y += title_size.y + gap;
        center_line(
            &painter,
            y,
            description_size,
            description,
            body_font,
            theme.ui.secondary_text,
        );
        y += description_size.y + gap;
        center_line(
            &painter,
            y,
            note_size,
            note,
            note_font,
            theme.ui.secondary_text,
        );

        response
    }
}
