//! Glassmorphism / acrylic material layer.
//!
//! The glass layer sits *below* all terminal content: terminal text, cursor,
//! selection and search overlays are always painted opaque with theme colors,
//! so the glass effect never blurs or recolors glyphs.
//!
//! Transparency is real (ARGB window surface, compositor-permitted). Blur is
//! delegated to whatever the platform exposes; see [`backend`] for details and
//! honest capability reporting.

pub mod backend;

pub use backend::GlassBackend;

use crate::config::GlassTint;
use eframe::egui::Color32;

/// RGB values of a tint preset (sRGB, 0-255).
pub fn tint_rgb(tint: GlassTint, custom: [u8; 3]) -> (u8, u8, u8) {
    match tint {
        GlassTint::Neutral => (128, 128, 128),
        GlassTint::Purple => (168, 85, 247),
        GlassTint::Pink => (236, 72, 153),
        GlassTint::Cyan => (34, 211, 238),
        GlassTint::Blue => (59, 130, 246),
        GlassTint::Green => (34, 197, 94),
        GlassTint::Custom => (custom[0], custom[1], custom[2]),
    }
}

/// Applies an alpha multiplier to a color, keeping its RGB unchanged.
pub fn with_alpha(color: Color32, opacity: f32) -> Color32 {
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Builds the glass material color for a layer.
///
/// The theme's own background color is the base so the material adapts to the
/// active theme (dark themes get dark glass, light themes get light glass),
/// then the configured tint is blended in at `tint_opacity`. The resulting
/// color is translucent at `opacity`, letting the desktop show through where
/// the compositor supports an ARGB window surface.
pub fn glass_fill(
    base: Color32,
    tint: GlassTint,
    custom: [u8; 3],
    tint_opacity: f32,
    opacity: f32,
) -> Color32 {
    let (tr, tg, tb) = tint_rgb(tint, custom);
    let mix = tint_opacity.clamp(0.0, 1.0);
    let r = base.r() as f32 * (1.0 - mix) + tr as f32 * mix;
    let g = base.g() as f32 * (1.0 - mix) + tg as f32 * mix;
    let b = base.b() as f32 * (1.0 - mix) + tb as f32 * mix;
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(r.round() as u8, g.round() as u8, b.round() as u8, alpha)
}
