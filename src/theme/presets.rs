use eframe::egui::Color32;

#[derive(Clone)]
pub struct TerminalTheme {
    pub background: Color32,
    pub foreground: Color32,
    pub cursor: Color32,
    pub selection_bg: Color32,
    pub selection_fg: Option<Color32>,
    pub ansi: [Color32; 16],
    pub search_highlight: Color32,
}

#[derive(Clone)]
pub struct UiTheme {
    pub background: Color32,
    pub panel: Color32,
    pub border: Color32,
    pub text: Color32,
    pub secondary_text: Color32,
    pub accent: Color32,
    pub tab_active: Color32,
    pub tab_inactive: Color32,
    pub divider: Color32,
}

#[derive(Clone)]
pub struct StatusTheme {
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
}

#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
    pub terminal: TerminalTheme,
    pub ui: UiTheme,
    pub status: StatusTheme,
}

fn ansi_from_rgb(arr: &[[u8; 3]; 16]) -> [Color32; 16] {
    let mut out: [Color32; 16] = [Color32::from_rgb(0, 0, 0); 16];
    for (i, rgb) in arr.iter().enumerate() {
        out[i] = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    }
    out
}

pub fn get_theme_names() -> Vec<&'static str> {
    vec![
        "orbit-dark",
        "orbit-light",
        "cyberpunk",
        "midnight-purple",
        "frost",
    ]
}

pub fn get_theme(name: &str) -> Theme {
    match name {
        "orbit-light" => orbit_light(),
        "cyberpunk" => cyberpunk(),
        "midnight-purple" => midnight_purple(),
        "frost" => frost(),
        _ => orbit_dark(),
    }
}

fn orbit_dark() -> Theme {
    let ansi = ansi_from_rgb(&[
        [0, 0, 0],       // black
        [192, 0, 0],     // red
        [0, 192, 0],     // green
        [192, 192, 0],   // yellow
        [0, 0, 192],     // blue
        [192, 0, 192],   // magenta
        [0, 192, 192],   // cyan
        [192, 192, 192], // white
        [96, 96, 96],    // bright black
        [255, 96, 96],   // bright red
        [96, 255, 96],   // bright green
        [255, 255, 96],  // bright yellow
        [96, 96, 255],   // bright blue
        [255, 96, 255],  // bright magenta
        [96, 255, 255],  // bright cyan
        [255, 255, 255], // bright white
    ]);

    Theme {
        name: "orbit-dark",
        terminal: TerminalTheme {
            background: Color32::from_rgb(10, 12, 14),
            foreground: Color32::from_rgb(220, 225, 230),
            cursor: Color32::from_rgb(120, 190, 255),
            selection_bg: Color32::from_rgb(45, 75, 105),
            selection_fg: None,
            ansi,
            search_highlight: Color32::from_rgb(95, 82, 32),
        },
        ui: UiTheme {
            background: Color32::from_rgb(8, 10, 12),
            panel: Color32::from_rgb(10, 12, 14),
            border: Color32::from_rgb(32, 38, 44),
            text: Color32::from_rgb(220, 225, 230),
            secondary_text: Color32::from_rgb(160, 165, 170),
            accent: Color32::from_rgb(120, 190, 255),
            tab_active: Color32::from_rgb(24, 30, 36),
            tab_inactive: Color32::from_rgb(18, 20, 22),
            divider: Color32::from_rgb(40, 44, 50),
        },
        status: StatusTheme {
            success: Color32::from_rgb(96, 255, 96),
            warning: Color32::from_rgb(255, 200, 96),
            error: Color32::from_rgb(255, 96, 96),
        },
    }
}

fn orbit_light() -> Theme {
    let ansi = ansi_from_rgb(&[
        [0, 0, 0],
        [170, 0, 0],
        [0, 128, 0],
        [170, 85, 0],
        [0, 0, 170],
        [170, 0, 170],
        [0, 170, 170],
        [170, 170, 170],
        [85, 85, 85],
        [255, 85, 85],
        [85, 255, 85],
        [255, 255, 85],
        [85, 85, 255],
        [255, 85, 255],
        [85, 255, 255],
        [255, 255, 255],
    ]);

    Theme {
        name: "orbit-light",
        terminal: TerminalTheme {
            background: Color32::from_rgb(245, 245, 245),
            foreground: Color32::from_rgb(20, 20, 20),
            cursor: Color32::from_rgb(10, 90, 200),
            selection_bg: Color32::from_rgb(200, 220, 240),
            selection_fg: None,
            ansi,
            search_highlight: Color32::from_rgb(200, 180, 140),
        },
        ui: UiTheme {
            background: Color32::from_rgb(250, 250, 250),
            panel: Color32::from_rgb(245, 245, 245),
            border: Color32::from_rgb(220, 220, 220),
            text: Color32::from_rgb(20, 20, 20),
            secondary_text: Color32::from_rgb(100, 100, 100),
            accent: Color32::from_rgb(10, 90, 200),
            tab_active: Color32::from_rgb(235, 235, 235),
            tab_inactive: Color32::from_rgb(245, 245, 245),
            divider: Color32::from_rgb(225, 225, 225),
        },
        status: StatusTheme {
            success: Color32::from_rgb(0, 160, 0),
            warning: Color32::from_rgb(200, 120, 0),
            error: Color32::from_rgb(200, 0, 0),
        },
    }
}

fn cyberpunk() -> Theme {
    let ansi = ansi_from_rgb(&[
        [8, 8, 8],
        [255, 64, 128],
        [64, 255, 128],
        [255, 200, 64],
        [64, 128, 255],
        [200, 64, 255],
        [64, 255, 255],
        [200, 200, 200],
        [100, 100, 100],
        [255, 120, 180],
        [120, 255, 180],
        [255, 230, 120],
        [120, 180, 255],
        [230, 120, 255],
        [120, 255, 255],
        [255, 255, 255],
    ]);

    Theme {
        name: "cyberpunk",
        terminal: TerminalTheme {
            background: Color32::from_rgb(6, 6, 12),
            foreground: Color32::from_rgb(220, 220, 255),
            cursor: Color32::from_rgb(255, 40, 120),
            selection_bg: Color32::from_rgb(40, 20, 60),
            selection_fg: None,
            ansi,
            search_highlight: Color32::from_rgb(255, 200, 64),
        },
        ui: UiTheme {
            background: Color32::from_rgb(8, 6, 10),
            panel: Color32::from_rgb(10, 8, 14),
            border: Color32::from_rgb(50, 10, 30),
            text: Color32::from_rgb(220, 220, 255),
            secondary_text: Color32::from_rgb(150, 150, 170),
            accent: Color32::from_rgb(255, 40, 120),
            tab_active: Color32::from_rgb(18, 12, 20),
            tab_inactive: Color32::from_rgb(12, 8, 14),
            divider: Color32::from_rgb(80, 20, 60),
        },
        status: StatusTheme {
            success: Color32::from_rgb(0, 200, 150),
            warning: Color32::from_rgb(255, 180, 80),
            error: Color32::from_rgb(255, 80, 120),
        },
    }
}

fn midnight_purple() -> Theme {
    let ansi = ansi_from_rgb(&[
        [3, 2, 10],
        [200, 90, 160],
        [90, 200, 160],
        [200, 180, 90],
        [90, 120, 200],
        [180, 90, 200],
        [90, 200, 200],
        [200, 200, 220],
        [100, 90, 120],
        [255, 140, 220],
        [140, 255, 220],
        [255, 255, 140],
        [140, 180, 255],
        [255, 140, 255],
        [140, 255, 255],
        [255, 255, 255],
    ]);

    Theme {
        name: "midnight-purple",
        terminal: TerminalTheme {
            background: Color32::from_rgb(12, 6, 22),
            foreground: Color32::from_rgb(230, 220, 255),
            cursor: Color32::from_rgb(180, 120, 255),
            selection_bg: Color32::from_rgb(50, 30, 70),
            selection_fg: None,
            ansi,
            search_highlight: Color32::from_rgb(200, 160, 220),
        },
        ui: UiTheme {
            background: Color32::from_rgb(14, 8, 26),
            panel: Color32::from_rgb(12, 6, 22),
            border: Color32::from_rgb(40, 20, 50),
            text: Color32::from_rgb(230, 220, 255),
            secondary_text: Color32::from_rgb(170, 160, 180),
            accent: Color32::from_rgb(180, 120, 255),
            tab_active: Color32::from_rgb(22, 12, 36),
            tab_inactive: Color32::from_rgb(16, 10, 28),
            divider: Color32::from_rgb(60, 30, 80),
        },
        status: StatusTheme {
            success: Color32::from_rgb(100, 255, 150),
            warning: Color32::from_rgb(255, 200, 120),
            error: Color32::from_rgb(255, 120, 160),
        },
    }
}

fn frost() -> Theme {
    let ansi = ansi_from_rgb(&[
        [2, 10, 14],
        [200, 80, 80],
        [80, 200, 150],
        [200, 200, 120],
        [120, 160, 255],
        [200, 140, 220],
        [120, 220, 220],
        [220, 240, 255],
        [120, 140, 160],
        [255, 140, 140],
        [140, 255, 200],
        [255, 255, 140],
        [140, 180, 255],
        [255, 140, 240],
        [140, 255, 255],
        [255, 255, 255],
    ]);

    Theme {
        name: "frost",
        terminal: TerminalTheme {
            background: Color32::from_rgb(6, 18, 22),
            foreground: Color32::from_rgb(220, 240, 255),
            cursor: Color32::from_rgb(160, 220, 255),
            selection_bg: Color32::from_rgb(30, 60, 80),
            selection_fg: None,
            ansi,
            search_highlight: Color32::from_rgb(140, 160, 190),
        },
        ui: UiTheme {
            background: Color32::from_rgb(10, 18, 22),
            panel: Color32::from_rgb(6, 18, 22),
            border: Color32::from_rgb(40, 60, 70),
            text: Color32::from_rgb(220, 240, 255),
            secondary_text: Color32::from_rgb(160, 180, 200),
            accent: Color32::from_rgb(160, 220, 255),
            tab_active: Color32::from_rgb(12, 22, 26),
            tab_inactive: Color32::from_rgb(8, 14, 18),
            divider: Color32::from_rgb(50, 80, 90),
        },
        status: StatusTheme {
            success: Color32::from_rgb(140, 255, 200),
            warning: Color32::from_rgb(255, 220, 140),
            error: Color32::from_rgb(255, 140, 140),
        },
    }
}
