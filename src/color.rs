use eframe::egui;

/// Perceived brightness of a color on a 0..255 scale.
pub fn luminance(color: egui::Color32) -> f32 {
    color.r() as f32 * 0.299 + color.g() as f32 * 0.587 + color.b() as f32 * 0.114
}

/// Linear RGB interpolation between two colors.
pub fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    egui::Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t).round() as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t).round() as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t).round() as u8,
    )
}

/// Whether a color reads as "light" (used to pick egui light vs dark visuals).
pub fn is_light_color(color: egui::Color32) -> bool {
    let red = color.r() as u32;
    let green = color.g() as u32;
    let blue = color.b() as u32;
    (red * 299 + green * 587 + blue * 114) >= 128_000
}
