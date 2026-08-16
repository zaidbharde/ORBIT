//! Platform capabilities for the glass material.
//!
//! This module probes the runtime Linux desktop and reports, honestly, what
//! the compositor and window backend actually support:
//!
//! - **Transparency**: requested via an ARGB window surface. winit falls back
//!   gracefully to an opaque surface when no compositor offers an alpha
//!   visual (X11), and Wayland surfaces are alpha-capable by construction.
//! - **Blur**: native compositor blur is only reachable where the window
//!   backend exposes it. On KWin over X11 the `_KDE_NET_WM_BLUR_BEHIND_REGION`
//!   property is honored by the compositor and is applied directly. On other
//!   compositors (e.g. GNOME Mutter) no window blur API exists, which is
//!   reported rather than faked.
//!
//! Everything here is failure-tolerant: any probe error degrades to
//! [`GlassBackend::Unsupported`] and ORBIT keeps running with an opaque
//! fallback.

use raw_window_handle::RawWindowHandle;
use std::sync::OnceLock;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::wrapper::ConnectionExt as _;

/// Which display session ORBIT is running in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Wayland,
    X11,
}

/// Blur capability of the active backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlurSupport {
    /// The compositor exposes a window blur API and it has been requested.
    Active,
    /// The compositor/window backend has no reachable blur API. The reason is
    /// reported so the limitation is never silently misrepresented.
    Unavailable { reason: String },
}

/// Detected glass backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlassBackend {
    LinuxNative {
        session: Session,
        compositor: Option<String>,
        blur: BlurSupport,
    },
    Unsupported {
        reason: String,
    },
}

impl GlassBackend {
    /// Human-readable summary used in the UI and startup log.
    pub fn describe(&self) -> String {
        match self {
            GlassBackend::LinuxNative {
                session,
                compositor,
                blur,
            } => {
                let session = match session {
                    Session::Wayland => "Wayland",
                    Session::X11 => "X11",
                };
                let compositor = compositor
                    .as_deref()
                    .map(|name| format!(", compositor \"{name}\""))
                    .unwrap_or_default();
                let blur = match blur {
                    BlurSupport::Active => "native blur active (KWin)".to_owned(),
                    BlurSupport::Unavailable { reason } => {
                        format!("blur unavailable ({reason})")
                    }
                };
                format!("ARGB transparency ({session}{compositor}); {blur}.")
            }
            GlassBackend::Unsupported { reason } => {
                format!("glass unsupported ({reason})")
            }
        }
    }

    /// True when the compositor honors the KWin X11 blur region property and
    /// ORBIT holds an X11 window it can attach the property to.
    pub fn x11_blur_available(&self) -> bool {
        matches!(
            self,
            GlassBackend::LinuxNative {
                session: Session::X11,
                blur: BlurSupport::Active,
                ..
            }
        )
    }
}

/// Probes the platform once per process and caches the result.
pub fn probe() -> &'static GlassBackend {
    static BACKEND: OnceLock<GlassBackend> = OnceLock::new();
    BACKEND.get_or_init(detect)
}

fn detect() -> GlassBackend {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();

    if !wayland && !x11 {
        return GlassBackend::Unsupported {
            reason: "no Wayland or X11 display detected".to_owned(),
        };
    }

    let compositor = if x11 { x11_compositor_name() } else { None };

    let session = if wayland {
        Session::Wayland
    } else {
        Session::X11
    };

    let blur = if matches!(session, Session::Wayland) {
        // winit 0.30 implements org_kde_kwin_blur for KWin, but eframe 0.32
        // does not expose it through its viewport API, and raw-window-handle
        // does not expose the Wayland surface object id needed to attach the
        // blur object from a separate connection. Honest limitation.
        BlurSupport::Unavailable {
            reason: "Wayland blur protocol not exposed by the window toolkit (eframe/winit)"
                .to_owned(),
        }
    } else if compositor
        .as_deref()
        .map(|name| name.to_ascii_lowercase().contains("kwin"))
        .unwrap_or(false)
    {
        BlurSupport::Active
    } else {
        BlurSupport::Unavailable {
            reason: compositor
                .as_deref()
                .map(|name| format!("compositor \"{name}\" does not support window blur"))
                .unwrap_or_else(|| "no X11 compositor detected".to_owned()),
        }
    };

    GlassBackend::LinuxNative {
        session,
        compositor,
        blur,
    }
}

/// The X11 window id of the given raw window handle, when it is an Xlib window.
pub fn x11_window_id(handle: &raw_window_handle::WindowHandle<'_>) -> Option<u64> {
    match handle.as_ref() {
        RawWindowHandle::Xlib(handle) => Some(handle.window),
        RawWindowHandle::Xcb(handle) => Some(handle.window.get().into()),
        _ => None,
    }
}

/// Applies the KWin `_KDE_NET_WM_BLUR_BEHIND_REGION` property so the
/// compositor blurs the desktop behind `window`. Cheap (a single X round
/// trip) and only called on window resize while blur is active.
pub fn apply_x11_blur_region(
    window: u64,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let (conn, _) = x11rb::connect(None).map_err(|err| err.to_string())?;
    let atom = conn
        .intern_atom(false, b"_KDE_NET_WM_BLUR_BEHIND_REGION")
        .map_err(|err| err.to_string())?
        .reply()
        .map_err(|err| err.to_string())?
        .atom;
    conn.change_property32(
        PropMode::REPLACE,
        Window::from(window as u32),
        atom,
        AtomEnum::CARDINAL,
        &[x, y, width, height],
    )
    .map_err(|err| err.to_string())?
    .check()
    .map_err(|err| err.to_string())?;
    conn.flush().map_err(|err| err.to_string())
}

/// Name of the running X11 window manager/compositor (via the EWMH
/// `_NET_SUPPORTING_WM_CHECK` root window property), if reachable.
fn x11_compositor_name() -> Option<String> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen_num].root;

    let wm_check_atom = conn
        .intern_atom(false, b"_NET_SUPPORTING_WM_CHECK")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let wm_name_atom = conn
        .intern_atom(false, b"_NET_WM_NAME")
        .ok()?
        .reply()
        .ok()?
        .atom;

    let wm_window: Window = conn
        .get_property(false, root, wm_check_atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()?;

    let name = conn
        .get_property(false, wm_window, wm_name_atom, AtomEnum::ANY, 0, 4096)
        .ok()?
        .reply()
        .ok()?;
    String::from_utf8(name.value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glass::with_alpha;
    use eframe::egui::Color32;

    /// egui stores colors premultiplied; recover the straight-alpha RGB.
    fn unmultiplied(color: Color32) -> (u8, u8, u8, u8) {
        let a = color.a();
        if a == 0 {
            return (0, 0, 0, 0);
        }
        let un = |v: u8| ((v as u16 * 255 + a as u16 / 2) / a as u16) as u8;
        (un(color.r()), un(color.g()), un(color.b()), a)
    }

    #[test]
    fn glass_fill_keeps_opaque_rgb_when_opacity_is_one() {
        let color = crate::glass::glass_fill(
            Color32::from_rgb(10, 12, 14),
            crate::config::GlassTint::Purple,
            [0, 0, 0],
            1.0,
            1.0,
        );
        assert_eq!(color.a(), 255);
        assert!(color.r() > 100, "tinted away from pure black");
    }

    #[test]
    fn glass_fill_respects_opacity() {
        let color = crate::glass::glass_fill(
            Color32::from_rgb(10, 12, 14),
            crate::config::GlassTint::Neutral,
            [0, 0, 0],
            0.0,
            0.5,
        );
        assert_eq!(unmultiplied(color), (10, 12, 14, 128));
    }

    #[test]
    fn custom_tint_uses_configured_rgb() {
        let color = crate::glass::glass_fill(
            Color32::BLACK,
            crate::config::GlassTint::Custom,
            [1, 2, 3],
            1.0,
            1.0,
        );
        assert_eq!((color.r(), color.g(), color.b()), (1, 2, 3));
    }

    #[test]
    fn with_alpha_keeps_rgb() {
        let color = with_alpha(Color32::from_rgb(200, 100, 50), 0.5);
        let (r, g, b, a) = unmultiplied(color);
        assert_eq!(a, 128);
        // 8-bit premultiplied storage may lose one unit of precision.
        assert!((r as i16 - 200).abs() <= 1);
        assert!((g as i16 - 100).abs() <= 1);
        assert!((b as i16 - 50).abs() <= 1);
    }

    #[test]
    fn backend_describe_is_never_empty() {
        let backend = GlassBackend::LinuxNative {
            session: Session::Wayland,
            compositor: Some("GNOME Shell".to_owned()),
            blur: BlurSupport::Unavailable {
                reason: "test".to_owned(),
            },
        };
        assert!(!backend.describe().is_empty());
    }

    #[test]
    fn x11_blur_availability_is_strict() {
        let unsupported = GlassBackend::LinuxNative {
            session: Session::Wayland,
            compositor: Some("KWin".to_owned()),
            blur: BlurSupport::Unavailable {
                reason: "test".to_owned(),
            },
        };
        assert!(!unsupported.x11_blur_available());
    }
}
