//! Vaughan wordmark and chrome polish helpers (pure ratatui, no extra crates).
//!
//! **Fixed (never themed):**
//! - **Banner** (`VAUGHAN`): neon blue → purple → hot pink
//! - **Splash slogan**: ghost typewriter — "the best wallet in the galaxy"
//! - **Wallet address**: Dioxus rainbow (`0x` / green / grey / orange / grey / purple)
//!
//! **Themed** (hidden hotkey `t` / `Ctrl+t` — not shown in the footer):
//! - Box / footer borders (faded or solid)
//! - Chrome text (accents, labels, titles)

use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::BorderType;
use ratatui::Frame;

const WORD: &str = "VAUGHAN";

type Rgb = (u8, u8, u8);

/// One stock theme: borders + chrome text (banner / address never read this).
struct ThemeSpec {
    id: &'static str,
    label: &'static str,
    /// When false, borders are flat and titles use solid title ink.
    fades: bool,
    accent: Rgb,
    body: Rgb,
    title: Rgb,
    box_a: Rgb,
    box_b: Rgb,
    foot_a: Rgb,
    foot_b: Rgb,
}

const fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Catalogue order = [`UiTheme`] discriminant order.
const THEME_CATALOG: &[ThemeSpec] = &[
    // --- colourful fades ---
    ThemeSpec {
        id: "pulse",
        label: "Pulse",
        fades: true,
        accent: (255, 255, 0),
        body: (255, 255, 255),
        title: (255, 255, 0),
        box_a: (255, 40, 40),
        box_b: (255, 20, 147),
        foot_a: (135, 206, 250),
        foot_b: (0, 139, 139),
    },
    ThemeSpec {
        id: "ocean",
        label: "Ocean",
        fades: true,
        accent: (0, 220, 255),
        body: (180, 230, 255),
        title: (135, 206, 250),
        box_a: (135, 206, 250),
        box_b: (0, 180, 220),
        foot_a: (0, 160, 160),
        foot_b: (20, 40, 120),
    },
    ThemeSpec {
        id: "ember",
        label: "Ember",
        fades: true,
        accent: (255, 180, 60),
        body: (255, 220, 180),
        title: (255, 140, 0),
        box_a: (255, 140, 0),
        box_b: (220, 20, 60),
        foot_a: (255, 200, 60),
        foot_b: (120, 70, 20),
    },
    ThemeSpec {
        id: "violet",
        label: "Violet",
        fades: true,
        accent: (255, 120, 220),
        body: (230, 200, 255),
        title: (200, 160, 255),
        box_a: (148, 0, 211),
        box_b: (255, 0, 180),
        foot_a: (200, 160, 255),
        foot_b: (75, 0, 130),
    },
    ThemeSpec {
        id: "forest",
        label: "Forest",
        fades: true,
        accent: (160, 255, 80),
        body: (200, 240, 180),
        title: (50, 205, 50),
        box_a: (50, 205, 50),
        box_b: (0, 150, 136),
        foot_a: (160, 230, 80),
        foot_b: (20, 80, 40),
    },
    // --- boring solids ---
    ThemeSpec {
        id: "slate",
        label: "Slate",
        fades: false,
        accent: (200, 205, 215),
        body: (150, 155, 165),
        title: (170, 175, 185),
        box_a: (100, 105, 115),
        box_b: (100, 105, 115),
        foot_a: (100, 105, 115),
        foot_b: (100, 105, 115),
    },
    ThemeSpec {
        id: "chalk",
        label: "Chalk",
        fades: false,
        accent: (230, 230, 230),
        body: (190, 190, 190),
        title: (210, 210, 210),
        box_a: (170, 170, 170),
        box_b: (170, 170, 170),
        foot_a: (170, 170, 170),
        foot_b: (170, 170, 170),
    },
    ThemeSpec {
        id: "paper",
        label: "Paper",
        fades: false,
        accent: (90, 70, 50),
        body: (120, 105, 90),
        title: (100, 85, 70),
        box_a: (140, 128, 112),
        box_b: (140, 128, 112),
        foot_a: (140, 128, 112),
        foot_b: (140, 128, 112),
    },
    ThemeSpec {
        id: "phosphor",
        label: "Phosphor",
        fades: false,
        accent: (0, 255, 70),
        body: (0, 180, 50),
        title: (0, 200, 60),
        box_a: (0, 140, 45),
        box_b: (0, 140, 45),
        foot_a: (0, 140, 45),
        foot_b: (0, 140, 45),
    },
    ThemeSpec {
        id: "amber",
        label: "Amber",
        fades: false,
        accent: (255, 176, 0),
        body: (200, 140, 0),
        title: (220, 150, 0),
        box_a: (160, 100, 0),
        box_b: (160, 100, 0),
        foot_a: (160, 100, 0),
        foot_b: (160, 100, 0),
    },
    ThemeSpec {
        id: "dim",
        label: "Dim",
        fades: false,
        accent: (140, 140, 140),
        body: (100, 100, 100),
        title: (120, 120, 120),
        box_a: (70, 70, 70),
        box_b: (70, 70, 70),
        foot_a: (70, 70, 70),
        foot_b: (70, 70, 70),
    },
    ThemeSpec {
        id: "stark",
        label: "Stark",
        fades: false,
        accent: (255, 255, 0),
        body: (255, 255, 255),
        title: (255, 255, 255),
        box_a: (220, 220, 220),
        box_b: (220, 220, 220),
        foot_a: (220, 220, 220),
        foot_b: (220, 220, 220),
    },
    // --- hybrids: boring → colour ---
    ThemeSpec {
        id: "mist",
        label: "Mist",
        fades: true,
        accent: (160, 200, 210),
        body: (140, 160, 170),
        title: (150, 180, 190),
        box_a: (120, 125, 130),
        box_b: (140, 190, 210),
        foot_a: (110, 115, 120),
        foot_b: (90, 150, 170),
    },
    ThemeSpec {
        id: "fogbloom",
        label: "FogBloom",
        fades: true,
        accent: (200, 170, 220),
        body: (170, 160, 180),
        title: (180, 165, 200),
        box_a: (130, 130, 135),
        box_b: (180, 150, 210),
        foot_a: (115, 115, 120),
        foot_b: (140, 110, 170),
    },
    ThemeSpec {
        id: "ashrose",
        label: "AshRose",
        fades: true,
        accent: (220, 150, 160),
        body: (180, 150, 155),
        title: (190, 140, 150),
        box_a: (90, 90, 95),
        box_b: (190, 110, 130),
        foot_a: (80, 80, 85),
        foot_b: (160, 90, 110),
    },
    ThemeSpec {
        id: "steelteal",
        label: "SteelTeal",
        fades: true,
        accent: (100, 200, 190),
        body: (140, 170, 175),
        title: (90, 180, 170),
        box_a: (95, 100, 110),
        box_b: (40, 160, 150),
        foot_a: (85, 90, 100),
        foot_b: (30, 120, 130),
    },
    ThemeSpec {
        id: "dustgold",
        label: "DustGold",
        fades: true,
        accent: (210, 180, 100),
        body: (180, 165, 130),
        title: (190, 160, 90),
        box_a: (120, 115, 105),
        box_b: (200, 160, 60),
        foot_a: (100, 95, 85),
        foot_b: (160, 120, 40),
    },
    ThemeSpec {
        id: "concretesky",
        label: "ConcreteSky",
        fades: true,
        accent: (150, 200, 240),
        body: (160, 170, 180),
        title: (140, 180, 220),
        box_a: (110, 110, 112),
        box_b: (100, 170, 230),
        foot_a: (95, 95, 98),
        foot_b: (70, 140, 200),
    },
    // --- random / playful fades ---
    ThemeSpec {
        id: "neonsoup",
        label: "NeonSoup",
        fades: true,
        accent: (200, 255, 0),
        body: (220, 180, 255),
        title: (180, 255, 80),
        box_a: (180, 255, 0),
        box_b: (160, 0, 255),
        foot_a: (255, 0, 200),
        foot_b: (0, 255, 180),
    },
    ThemeSpec {
        id: "candy",
        label: "Candy",
        fades: true,
        accent: (255, 120, 180),
        body: (255, 200, 220),
        title: (255, 140, 190),
        box_a: (255, 105, 180),
        box_b: (0, 220, 255),
        foot_a: (255, 160, 100),
        foot_b: (255, 80, 160),
    },
    ThemeSpec {
        id: "aurora",
        label: "Aurora",
        fades: true,
        accent: (80, 255, 200),
        body: (180, 255, 220),
        title: (100, 255, 180),
        box_a: (0, 255, 140),
        box_b: (255, 0, 200),
        foot_a: (40, 200, 255),
        foot_b: (180, 80, 255),
    },
    ThemeSpec {
        id: "sunset",
        label: "Sunset",
        fades: true,
        accent: (255, 160, 80),
        body: (255, 200, 160),
        title: (255, 120, 60),
        box_a: (255, 90, 20),
        box_b: (120, 40, 160),
        foot_a: (255, 180, 60),
        foot_b: (80, 30, 100),
    },
    ThemeSpec {
        id: "icefire",
        label: "Icefire",
        fades: true,
        accent: (255, 100, 80),
        body: (180, 220, 255),
        title: (120, 200, 255),
        box_a: (140, 210, 255),
        box_b: (255, 40, 40),
        foot_a: (100, 180, 255),
        foot_b: (200, 30, 30),
    },
    ThemeSpec {
        id: "toxic",
        label: "Toxic",
        fades: true,
        accent: (200, 255, 40),
        body: (180, 220, 100),
        title: (160, 255, 0),
        box_a: (180, 255, 0),
        box_b: (255, 20, 147),
        foot_a: (120, 200, 0),
        foot_b: (80, 40, 60),
    },
    ThemeSpec {
        id: "grapesoda",
        label: "GrapeSoda",
        fades: true,
        accent: (180, 140, 255),
        body: (160, 150, 220),
        title: (160, 120, 255),
        box_a: (90, 40, 160),
        box_b: (40, 100, 220),
        foot_a: (70, 30, 140),
        foot_b: (20, 80, 180),
    },
    ThemeSpec {
        id: "honeydew",
        label: "Honeydew",
        fades: true,
        accent: (200, 230, 80),
        body: (220, 230, 160),
        title: (180, 220, 100),
        box_a: (180, 230, 140),
        box_b: (255, 220, 60),
        foot_a: (140, 190, 100),
        foot_b: (200, 160, 40),
    },
    ThemeSpec {
        id: "magma",
        label: "Magma",
        fades: true,
        accent: (255, 200, 40),
        body: (255, 160, 80),
        title: (255, 120, 20),
        box_a: (120, 20, 10),
        box_b: (255, 200, 0),
        foot_a: (180, 40, 10),
        foot_b: (255, 100, 0),
    },
    ThemeSpec {
        id: "glitch",
        label: "Glitch",
        fades: true,
        accent: (0, 255, 255),
        body: (255, 0, 255),
        title: (0, 255, 200),
        box_a: (255, 0, 255),
        box_b: (0, 255, 255),
        foot_a: (200, 0, 255),
        foot_b: (0, 200, 255),
    },
    // --- more solids / oddballs ---
    ThemeSpec {
        id: "olive",
        label: "Olive",
        fades: false,
        accent: (180, 190, 100),
        body: (140, 150, 90),
        title: (160, 170, 95),
        box_a: (100, 110, 60),
        box_b: (100, 110, 60),
        foot_a: (100, 110, 60),
        foot_b: (100, 110, 60),
    },
    ThemeSpec {
        id: "navy",
        label: "Navy",
        fades: false,
        accent: (140, 180, 255),
        body: (100, 140, 200),
        title: (120, 160, 230),
        box_a: (30, 50, 100),
        box_b: (30, 50, 100),
        foot_a: (30, 50, 100),
        foot_b: (30, 50, 100),
    },
    ThemeSpec {
        id: "mauve",
        label: "Mauve",
        fades: false,
        accent: (210, 160, 190),
        body: (170, 140, 160),
        title: (190, 150, 175),
        box_a: (130, 100, 120),
        box_b: (130, 100, 120),
        foot_a: (130, 100, 120),
        foot_b: (130, 100, 120),
    },
    ThemeSpec {
        id: "copper",
        label: "Copper",
        fades: true,
        accent: (230, 150, 80),
        body: (200, 140, 100),
        title: (210, 130, 70),
        box_a: (140, 80, 40),
        box_b: (220, 140, 60),
        foot_a: (100, 60, 30),
        foot_b: (180, 100, 50),
    },
    ThemeSpec {
        id: "mintchip",
        label: "MintChip",
        fades: true,
        accent: (120, 255, 200),
        body: (180, 220, 200),
        title: (100, 230, 180),
        box_a: (60, 180, 140),
        box_b: (40, 40, 40),
        foot_a: (80, 200, 160),
        foot_b: (60, 60, 60),
    },
    ThemeSpec {
        id: "lava",
        label: "Lava",
        fades: true,
        accent: (255, 80, 40),
        body: (255, 160, 100),
        title: (255, 100, 50),
        box_a: (40, 10, 10),
        box_b: (255, 60, 0),
        foot_a: (80, 20, 10),
        foot_b: (255, 120, 0),
    },
];

/// Stock UI themes for boxes / footer / chrome text (banner + address stay fixed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum UiTheme {
    Pulse = 0,
    Ocean = 1,
    Ember = 2,
    Violet = 3,
    Forest = 4,
    Slate = 5,
    Chalk = 6,
    Paper = 7,
    Phosphor = 8,
    Amber = 9,
    Dim = 10,
    Stark = 11,
    Mist = 12,
    FogBloom = 13,
    AshRose = 14,
    SteelTeal = 15,
    DustGold = 16,
    ConcreteSky = 17,
    NeonSoup = 18,
    Candy = 19,
    Aurora = 20,
    Sunset = 21,
    Icefire = 22,
    Toxic = 23,
    GrapeSoda = 24,
    Honeydew = 25,
    Magma = 26,
    Glitch = 27,
    Olive = 28,
    Navy = 29,
    Mauve = 30,
    Copper = 31,
    MintChip = 32,
    Lava = 33,
}

impl UiTheme {
    /// All stock themes in cycle order.
    pub const ALL: [UiTheme; 34] = [
        UiTheme::Pulse,
        UiTheme::Ocean,
        UiTheme::Ember,
        UiTheme::Violet,
        UiTheme::Forest,
        UiTheme::Slate,
        UiTheme::Chalk,
        UiTheme::Paper,
        UiTheme::Phosphor,
        UiTheme::Amber,
        UiTheme::Dim,
        UiTheme::Stark,
        UiTheme::Mist,
        UiTheme::FogBloom,
        UiTheme::AshRose,
        UiTheme::SteelTeal,
        UiTheme::DustGold,
        UiTheme::ConcreteSky,
        UiTheme::NeonSoup,
        UiTheme::Candy,
        UiTheme::Aurora,
        UiTheme::Sunset,
        UiTheme::Icefire,
        UiTheme::Toxic,
        UiTheme::GrapeSoda,
        UiTheme::Honeydew,
        UiTheme::Magma,
        UiTheme::Glitch,
        UiTheme::Olive,
        UiTheme::Navy,
        UiTheme::Mauve,
        UiTheme::Copper,
        UiTheme::MintChip,
        UiTheme::Lava,
    ];

    fn spec(self) -> &'static ThemeSpec {
        &THEME_CATALOG[self as usize]
    }

    /// Short label.
    pub fn label(self) -> &'static str {
        self.spec().label
    }

    /// Stable id written to disk (`ui-theme` file).
    pub fn id(self) -> &'static str {
        self.spec().id
    }

    /// Parse a persisted id (case-insensitive).
    pub fn parse(raw: &str) -> Option<Self> {
        let key = raw.trim().to_ascii_lowercase();
        Self::ALL.iter().copied().find(|t| t.spec().id == key)
    }

    fn from_index(i: usize) -> Self {
        Self::ALL[i % Self::ALL.len()]
    }

    /// Whether borders / titles lerp across two colours.
    pub fn fades(self) -> bool {
        self.spec().fades
    }

    fn ink(self) -> ThemeInk {
        let s = self.spec();
        ThemeInk {
            accent: rgb(s.accent),
            body: rgb(s.body),
            title: rgb(s.title),
        }
    }
}

/// Text inks for chrome (keys, values, labels, solid titles).
#[derive(Debug, Clone, Copy)]
struct ThemeInk {
    accent: Color,
    body: Color,
    title: Color,
}

static THEME_IDX: AtomicUsize = AtomicUsize::new(UiTheme::Pulse as usize);

/// Active stock theme.
pub fn current_theme() -> UiTheme {
    UiTheme::from_index(THEME_IDX.load(Ordering::Relaxed))
}

/// Set the active stock theme (tests / restore).
pub fn set_theme(theme: UiTheme) {
    THEME_IDX.store(theme as usize, Ordering::Relaxed);
}

/// Advance to the next stock theme, persist it as the new default, and return it.
pub fn cycle_theme() -> UiTheme {
    loop {
        let cur = THEME_IDX.load(Ordering::Relaxed);
        let next = (cur + 1) % UiTheme::ALL.len();
        if THEME_IDX
            .compare_exchange(cur, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            let theme = UiTheme::from_index(next);
            let _ = persist_theme(theme);
            return theme;
        }
    }
}

/// Path to the persisted theme id: `<data_dir>/vaughan-cli/ui-theme`.
pub fn theme_file_path() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join("vaughan-cli").join("ui-theme"))
}

/// Write `theme` so the next launch restores it.
pub fn persist_theme(theme: UiTheme) -> std::io::Result<()> {
    let path = theme_file_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not determine user data directory",
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, theme.id())
}

/// Load the last-selected theme from disk (no-op if missing / invalid).
pub fn load_persisted_theme() {
    let Some(path) = theme_file_path() else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    if let Some(theme) = UiTheme::parse(&raw) {
        set_theme(theme);
    }
}

/// Accent colour for keys, values, and focus cues (theme-dependent).
pub fn accent_color() -> Color {
    current_theme().ink().accent
}

/// Body / label colour for chrome text (theme-dependent).
pub fn body_color() -> Color {
    current_theme().ink().body
}

/// Title colour for solid (non-fade) themes.
pub fn title_color() -> Color {
    current_theme().ink().title
}

/// Which gradient to use for text or box borders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadePalette {
    /// Top wordmark / splash art: always neon blue → purple → hot pink (not themed).
    Banner,
    /// Panels / titles: themed.
    Box,
    /// Bottom key chips: themed.
    Footer,
}

/// Banner stops: neon blue → neon purple → hot pink (fixed).
const FADE_BANNER: [(f64, Rgb); 3] = [
    (0.0, (0, 245, 255)),  // neon blue
    (0.5, (180, 0, 255)),  // neon purple
    (1.0, (255, 20, 147)), // hot pink
];

/// FIGlet ANSI Shadow–style `VAUGHAN` (Claude Code–like splash). Pure data, no crate.
const LOGO_ART: &[&str] = &[
    "██╗   ██╗ █████╗ ██╗   ██╗ ██████╗ ██╗  ██╗ █████╗ ███╗   ██╗",
    "██║   ██║██╔══██╗██║   ██║██╔════╝ ██║  ██║██╔══██╗████╗  ██║",
    "██║   ██║███████║██║   ██║██║  ███╗███████║███████║██╔██╗ ██║",
    "╚██╗ ██╔╝██╔══██║██║   ██║██║   ██║██╔══██║██╔══██║██║╚██╗██║",
    " ╚████╔╝ ██║  ██║╚██████╔╝╚██████╔╝██║  ██║██║  ██║██║ ╚████║",
    "  ╚═══╝  ╚═╝  ╚═╝ ╚═════╝  ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝  ╚═══╝",
];

fn lerp(a: u8, b: u8, t: f64) -> u8 {
    let v = f64::from(a) + (f64::from(b) - f64::from(a)) * t;
    v.round().clamp(0.0, 255.0) as u8
}

fn lerp_stops(stops: &[(f64, Rgb)], t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (i, (t0, c0)) = stops
        .iter()
        .enumerate()
        .rev()
        .find(|(_, (stop, _))| t >= *stop)
        .map(|(i, s)| (i, *s))
        .unwrap_or((0, stops[0]));
    let (t1, c1) = stops.get(i + 1).copied().unwrap_or((1.0, c0));
    let local = if (t1 - t0).abs() < f64::EPSILON {
        0.0
    } else {
        (t - t0) / (t1 - t0)
    };
    Color::Rgb(
        lerp(c0.0, c1.0, local),
        lerp(c0.1, c1.1, local),
        lerp(c0.2, c1.2, local),
    )
}

/// Colour at normalised position `t` ∈ [0, 1] along the box fade.
pub fn fade_color(t: f64) -> Color {
    fade_color_with(FadePalette::Box, t)
}

/// Colour at normalised position `t` ∈ [0, 1] along `palette`.
pub fn fade_color_with(palette: FadePalette, t: f64) -> Color {
    match palette {
        FadePalette::Banner => lerp_stops(&FADE_BANNER, t),
        FadePalette::Box => {
            let s = current_theme().spec();
            lerp_stops(&[(0.0, s.box_a), (1.0, s.box_b)], t)
        }
        FadePalette::Footer => {
            let s = current_theme().spec();
            lerp_stops(&[(0.0, s.foot_a), (1.0, s.foot_b)], t)
        }
    }
}

/// Vaughan-Dioxus address palette (`#808080` / `#33cc33` / `#ff9933` / `#b24cff`).
const ADDR_GREY: Color = Color::Rgb(128, 128, 128);
const ADDR_GREEN: Color = Color::Rgb(51, 204, 51);
const ADDR_ORANGE: Color = Color::Rgb(255, 153, 51);
const ADDR_PURPLE: Color = Color::Rgb(178, 76, 255);

/// Full-width top banner: `/` to the left edge, centered `VAUGHAN`, `\` to the right,
/// with a neon-blue→purple→hot-pink colour fade across the row.
///
/// Example at width 27: `//////////VAUGHAN\\\\\\\\\\`
pub fn logo_banner(width: u16) -> Line<'static> {
    let width = width as usize;
    let text = if width == 0 {
        String::new()
    } else if width <= WORD.len() {
        WORD.chars().take(width).collect()
    } else {
        let side = width - WORD.len();
        let left = side / 2;
        let right = side - left;
        format!("{}{}{}", "/".repeat(left), WORD, "\\".repeat(right))
    };
    fade_line_with(&text, FadePalette::Banner)
}

/// Natural width (columns) of [`logo_art_lines`] before centring.
pub fn logo_art_width() -> usize {
    LOGO_ART
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0)
}

/// Row count of the multi-line splash wordmark.
pub fn logo_art_height() -> u16 {
    LOGO_ART.len() as u16
}

/// Multi-line FIGlet-style `VAUGHAN` splash, centred in `width`, with a
/// neon-blue→purple→hot-pink fade across each row.
///
/// When `width` is narrower than the glyph, falls back to a single [`logo_banner`]
/// line so narrow terminals stay usable.
pub fn logo_art_lines(width: u16) -> Vec<Line<'static>> {
    let art_w = logo_art_width();
    if width == 0 {
        return Vec::new();
    }
    if (width as usize) < art_w {
        return vec![logo_banner(width)];
    }

    let pad = (width as usize - art_w) / 2;
    LOGO_ART
        .iter()
        .map(|row| {
            let mut spans = Vec::with_capacity(pad + art_w);
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
            // Right-pad short rows so the fade spans the full glyph width.
            let padded = format!("{row:<art_w$}");
            for (i, ch) in padded.chars().enumerate() {
                let t = if art_w <= 1 {
                    0.0
                } else {
                    i as f64 / (art_w - 1) as f64
                };
                spans.push(Span::styled(
                    ch.to_string(),
                    Style::default()
                        .fg(fade_color_with(FadePalette::Banner, t))
                        .add_modifier(Modifier::BOLD),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

/// Splash tagline typed by a ghost in the terminal.
pub const SPLASH_SLOGAN: &str = "the best wallet in the galaxy";

/// Ghost typewriter for [`SPLASH_SLOGAN`], driven by the UI tick (~80 ms).
///
/// Types one character at a time, then holds with a blinking caret (plays once).
pub fn typing_slogan(tick: u64) -> Line<'static> {
    const TICKS_PER_CHAR: u64 = 2; // ~160 ms per glyph

    let chars: Vec<char> = SPLASH_SLOGAN.chars().collect();
    let type_phase = (chars.len() as u64).saturating_mul(TICKS_PER_CHAR);
    let visible = if tick < type_phase {
        ((tick / TICKS_PER_CHAR) as usize).min(chars.len())
    } else {
        chars.len()
    };

    let typed: String = chars.into_iter().take(visible).collect();
    let caret = if (tick / 4).is_multiple_of(2) {
        "▌"
    } else {
        " "
    };

    let ghost = Style::default()
        .fg(Color::Rgb(170, 150, 220))
        .add_modifier(Modifier::DIM | Modifier::ITALIC);
    let caret_style = Style::default()
        .fg(Color::Rgb(0, 245, 255))
        .add_modifier(Modifier::BOLD);

    Line::from(vec![
        Span::styled(typed, ghost),
        Span::styled(caret.to_string(), caret_style),
    ])
}

/// One [`Line`] for panel titles: box fade, or solid title ink on no-fade themes.
pub fn fade_line(text: &str) -> Line<'static> {
    fade_line_with(text, FadePalette::Box)
}

/// One [`Line`] faded with the given palette.
pub fn fade_line_with(text: &str, palette: FadePalette) -> Line<'static> {
    Line::from(fade_spans_with(text, palette))
}

/// One styled span per character, colour lerped along the box fade.
pub fn fade_spans(text: &str) -> Vec<Span<'static>> {
    fade_spans_with(text, FadePalette::Box)
}

/// One styled span per character along `palette`.
pub fn fade_spans_with(text: &str, palette: FadePalette) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    // Solid themes: titles use a flat ink (not a border lerp).
    let solid_title = palette != FadePalette::Banner && !current_theme().fades();
    let solid_fg = title_color();
    chars
        .into_iter()
        .enumerate()
        .map(|(i, ch)| {
            let fg = if solid_title {
                solid_fg
            } else {
                let t = if n <= 1 {
                    0.0
                } else {
                    i as f64 / (n - 1) as f64
                };
                fade_color_with(palette, t)
            };
            Span::styled(
                ch.to_string(),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// Focus cue for panel titles (theme accent), used by inputs and selection.
pub fn focus_title(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(accent_color())
            .add_modifier(Modifier::BOLD),
    ))
}

/// Colour for hex body index `i` (0..40), matching Vaughan-Dioxus `ColoredAddressText`.
fn address_body_color(i: usize) -> Color {
    match i {
        0..=4 => ADDR_GREEN,
        5..=17 => ADDR_GREY,
        18..=22 => ADDR_ORANGE,
        23..=34 => ADDR_GREY,
        35..=39 => ADDR_PURPLE,
        _ => ADDR_GREY,
    }
}

fn addr_style(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// Rainbow-segmented wallet address spans (Dioxus `ColoredAddressText` layout).
///
/// Short / non-hex placeholders render in grey.
pub fn colored_address_spans(address: &str) -> Vec<Span<'static>> {
    let addr = address.trim();
    let body = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .unwrap_or(addr);
    if body.len() < 40 {
        return vec![Span::styled(addr.to_string(), addr_style(ADDR_GREY))];
    }

    let mut spans = Vec::with_capacity(6);
    if addr.starts_with("0x") || addr.starts_with("0X") {
        spans.push(Span::styled("0x".to_string(), addr_style(ADDR_GREY)));
    }
    // Segment by Dioxus ranges on the 40-char body.
    let parts: &[(usize, usize, Color)] = &[
        (0, 5, ADDR_GREEN),
        (5, 18, ADDR_GREY),
        (18, 23, ADDR_ORANGE),
        (23, 35, ADDR_GREY),
        (35, 40, ADDR_PURPLE),
    ];
    for &(start, end, color) in parts {
        spans.push(Span::styled(
            body[start..end].to_string(),
            addr_style(color),
        ));
    }
    spans
}

/// Rainbow address that middle-ellipsizes to `budget` columns when needed.
pub fn colored_address_fitted(address: &str, budget: usize) -> Vec<Span<'static>> {
    if budget == 0 {
        return Vec::new();
    }
    let addr = address.trim();
    let chars: Vec<char> = addr.chars().collect();
    if chars.len() <= budget {
        return colored_address_spans(addr);
    }
    if budget <= 5 {
        let s: String = chars.into_iter().take(budget).collect();
        return vec![Span::styled(s, addr_style(ADDR_GREY))];
    }

    let keep = budget.saturating_sub(1) / 2;
    let right = budget.saturating_sub(keep + 1);
    let mut spans = Vec::new();

    // Prefix: colour by position in the original string (skipping optional "0x").
    let has_0x = addr.starts_with("0x") || addr.starts_with("0X");
    let body_offset = if has_0x { 2 } else { 0 };
    for (i, ch) in chars.iter().take(keep).enumerate() {
        let color = if has_0x && i < 2 {
            ADDR_GREY
        } else {
            address_body_color(i.saturating_sub(body_offset))
        };
        spans.push(Span::styled(ch.to_string(), addr_style(color)));
    }
    spans.push(Span::styled("…".to_string(), addr_style(ADDR_GREY)));
    // Suffix: map from the end of the original address.
    let start = chars.len().saturating_sub(right);
    for (j, ch) in chars.iter().skip(start).enumerate() {
        let orig_i = start + j;
        let color = if has_0x && orig_i < 2 {
            ADDR_GREY
        } else {
            address_body_color(orig_i.saturating_sub(body_offset))
        };
        spans.push(Span::styled(ch.to_string(), addr_style(color)));
    }
    spans
}

/// Column where `AUGHA` starts in [`logo_banner`] (skips the leading `V`).
pub fn wordmark_augha_start(width: u16) -> usize {
    let w = width as usize;
    if w == 0 {
        return 0;
    }
    if w <= WORD.len() {
        // Truncated wordmark: `A` of `VAUGHAN` is still index 1 when present.
        1.min(w.saturating_sub(1))
    } else {
        let left = (w - WORD.len()) / 2;
        left + 1
    }
}

/// Index of the orange segment within a full `0x` + 40-hex address (`body[18..23]`).
const ORANGE_IN_FULL_ADDR: usize = 2 + 18; // after `0x`

/// Colour-coded address with left padding so the orange mid-segment sits under
/// `AUGHA` in the wordmark (same column as [`wordmark_augha_start`]).
///
/// Falls back to a fitted / centred layout when the terminal is too narrow.
pub fn colored_address_under_augha(address: &str, width: u16) -> Line<'static> {
    let budget = width as usize;
    if budget == 0 {
        return Line::from("");
    }

    let addr = address.trim();
    let body = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .unwrap_or(addr);
    let has_0x = addr.starts_with("0x") || addr.starts_with("0X");
    let augha = wordmark_augha_start(width);

    // Full eth address: pad so orange (5 chars) lines up under AUGHA (5 chars).
    if has_0x && body.len() >= 40 {
        let full_len = 42; // 0x + 40
        if full_len <= budget {
            let mut pad = augha.saturating_sub(ORANGE_IN_FULL_ADDR);
            if pad + full_len > budget {
                pad = budget.saturating_sub(full_len);
            }
            let mut spans = Vec::new();
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
            spans.extend(colored_address_spans(addr));
            return Line::from(spans);
        }
    }

    // Narrow / placeholder: fit then centre under the wordmark.
    let fitted = colored_address_fitted(addr, budget);
    let text_w: usize = fitted.iter().map(|s| s.content.chars().count()).sum();
    let pad = budget.saturating_sub(text_w) / 2;
    let mut spans = Vec::new();
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    spans.extend(fitted);
    Line::from(spans)
}

/// Draw a square-corner box with a left→right **box** fade (red → hot pink).
///
/// Optional `title` is painted into the top border (starting two cells in).
/// Returns the inner content [`Rect`] (empty if the area is too small).
pub fn render_faded_box(frame: &mut Frame, area: Rect, title: Option<Line<'_>>) -> Rect {
    render_faded_box_with(frame, area, title, FadePalette::Box)
}

/// Draw a square-corner box with the given fade palette on the borders.
pub fn render_faded_box_with(
    frame: &mut Frame,
    area: Rect,
    title: Option<Line<'_>>,
    palette: FadePalette,
) -> Rect {
    render_faded_box_buf_with(frame.buffer_mut(), area, title, palette)
}

/// Same as [`render_faded_box`] but writes into a [`Buffer`] (for tests).
pub fn render_faded_box_buf(buf: &mut Buffer, area: Rect, title: Option<Line<'_>>) -> Rect {
    render_faded_box_buf_with(buf, area, title, FadePalette::Box)
}

/// Same as [`render_faded_box_with`] but writes into a [`Buffer`].
pub fn render_faded_box_buf_with(
    buf: &mut Buffer,
    area: Rect,
    title: Option<Line<'_>>,
    palette: FadePalette,
) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }

    let set = BorderType::border_symbols(BorderType::Plain);
    let w = area.width;
    let h = area.height;

    let x_t = |x: u16| -> f64 {
        if w <= 1 {
            0.0
        } else {
            f64::from(x) / f64::from(w - 1)
        }
    };
    let y_t = |y: u16| -> f64 {
        if h <= 1 {
            0.0
        } else {
            f64::from(y) / f64::from(h - 1)
        }
    };

    // Top edge (square corners).
    for dx in 0..w {
        let style = Style::default().fg(fade_color_with(palette, x_t(dx)));
        let symbol = if dx == 0 {
            set.top_left
        } else if dx == w - 1 {
            set.top_right
        } else {
            set.horizontal_top
        };
        buf[(area.x + dx, area.y)]
            .set_symbol(symbol)
            .set_style(style);
    }

    // Bottom edge.
    if h > 1 {
        let by = area.y + h - 1;
        for dx in 0..w {
            let style = Style::default().fg(fade_color_with(palette, x_t(dx)));
            let symbol = if dx == 0 {
                set.bottom_left
            } else if dx == w - 1 {
                set.bottom_right
            } else {
                set.horizontal_bottom
            };
            buf[(area.x + dx, by)].set_symbol(symbol).set_style(style);
        }
    }

    // Vertical edges: left near start of palette, right near end.
    if h > 2 {
        for dy in 1..h - 1 {
            let t = y_t(dy);
            let left_c = fade_color_with(palette, t * 0.35);
            let right_c = fade_color_with(palette, 0.65 + t * 0.35);
            buf[(area.x, area.y + dy)]
                .set_symbol(set.vertical_left)
                .set_style(Style::default().fg(left_c));
            if w > 1 {
                buf[(area.x + w - 1, area.y + dy)]
                    .set_symbol(set.vertical_right)
                    .set_style(Style::default().fg(right_c));
            }
        }
    }

    // Title inset on the top border.
    if let Some(title) = title {
        paint_title_on_top(buf, area, title);
    }

    if w < 3 || h < 3 {
        return Rect::default();
    }
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: w - 2,
        height: h - 2,
    }
}

fn paint_title_on_top(buf: &mut Buffer, area: Rect, title: Line<'_>) {
    if area.width < 4 {
        return;
    }
    // Leave corner + one horizontal cell free on each side when possible.
    let start_x = area.x.saturating_add(2);
    let max_x = area.x.saturating_add(area.width.saturating_sub(2));
    let mut x = start_x;
    for span in title.spans {
        let style = span.style;
        for ch in span.content.chars() {
            if x >= max_x {
                return;
            }
            buf[(x, area.y)].set_char(ch).set_style(style);
            x = x.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use std::sync::Mutex;

    /// Theme state is process-global; serialise tests that mutate it.
    static THEME_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn with_theme<R>(theme: UiTheme, f: impl FnOnce() -> R) -> R {
        let _guard = THEME_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = current_theme();
        set_theme(theme);
        let out = f();
        set_theme(prev);
        out
    }

    #[test]
    fn banner_centers_word_with_slashes_to_edges() {
        let line = logo_banner(27);
        let s: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert_eq!(s, r"//////////VAUGHAN\\\\\\\\\\");
        assert_eq!(s.len(), 27);
        assert!(s.starts_with('/'));
        assert!(s.ends_with('\\'));
        assert!(s.contains(WORD));
    }

    #[test]
    fn banner_fills_exact_width() {
        for w in [1u16, 7, 8, 40, 80, 120] {
            let line = logo_banner(w);
            let s: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
            assert_eq!(s.chars().count(), w as usize, "width {w}");
        }
    }

    #[test]
    fn banner_fade_runs_neon_blue_to_pink() {
        let line = logo_banner(40);
        assert!(line.spans.len() >= 2);
        let first = match line.spans[0].style.fg {
            Some(Color::Rgb(r, g, b)) => (r, g, b),
            other => panic!("expected Rgb at start, got {other:?}"),
        };
        let mid = match line.spans[line.spans.len() / 2].style.fg {
            Some(Color::Rgb(r, g, b)) => (r, g, b),
            other => panic!("expected Rgb at mid, got {other:?}"),
        };
        let last = match line.spans.last().unwrap().style.fg {
            Some(Color::Rgb(r, g, b)) => (r, g, b),
            other => panic!("expected Rgb at end, got {other:?}"),
        };
        assert_eq!(first, (0, 245, 255), "start should be neon blue");
        assert!(
            mid.0 > 100 && mid.2 > 180,
            "mid should lean purple: {mid:?}"
        );
        assert_eq!(last, (255, 20, 147), "end should be hot pink");
        let colours: Vec<_> = line.spans.iter().filter_map(|s| s.style.fg).collect();
        let unique: std::collections::HashSet<_> = colours.iter().collect();
        assert!(
            unique.len() > 4,
            "expected a multi-stop fade, got {}",
            unique.len()
        );
    }

    #[test]
    fn logo_art_lines_centred_with_banner_fade() {
        let width = (logo_art_width() + 10) as u16;
        let lines = logo_art_lines(width);
        assert_eq!(lines.len(), logo_art_height() as usize);
        let first = &lines[0];
        let s: String = first.spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(s.contains('█'), "expected block glyph: {s}");
        assert_eq!(s.chars().count(), pad_len(width) + logo_art_width());
        let coloured: Vec<_> = first
            .spans
            .iter()
            .filter(|sp| sp.style.fg.is_some())
            .collect();
        assert!(!coloured.is_empty());
        assert_eq!(coloured[0].style.fg, Some(Color::Rgb(0, 245, 255)));
        assert_eq!(
            coloured.last().unwrap().style.fg,
            Some(Color::Rgb(255, 20, 147))
        );
    }

    fn pad_len(width: u16) -> usize {
        (width as usize - logo_art_width()) / 2
    }

    #[test]
    fn logo_art_falls_back_when_narrow() {
        let lines = logo_art_lines(20);
        assert_eq!(lines.len(), 1);
        let s: String = lines[0]
            .spans
            .iter()
            .map(|sp| sp.content.as_ref())
            .collect();
        assert!(s.contains(WORD), "{s}");
        assert_eq!(s.chars().count(), 20);
    }

    #[test]
    fn typing_slogan_reveals_progressively() {
        let early: String = typing_slogan(0)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        // First frame: caret only (no letters yet).
        assert!(!early.contains("best"), "{early}");

        let mid_tick = 2 * 10; // ~10 chars in
        let mid: String = typing_slogan(mid_tick)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(mid.starts_with("the best"), "{mid}");
        assert!(mid.len() < SPLASH_SLOGAN.len() + 2, "{mid}");

        let done_tick = 2 * SPLASH_SLOGAN.chars().count() as u64 + 5;
        let done: String = typing_slogan(done_tick)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(done.contains(SPLASH_SLOGAN), "{done}");
    }

    #[test]
    fn box_fade_is_red_to_hot_pink() {
        with_theme(UiTheme::Pulse, || {
            let start = match fade_color_with(FadePalette::Box, 0.0) {
                Color::Rgb(r, g, b) => (r, g, b),
                other => panic!("{other:?}"),
            };
            let end = match fade_color_with(FadePalette::Box, 1.0) {
                Color::Rgb(r, g, b) => (r, g, b),
                other => panic!("{other:?}"),
            };
            assert_eq!(start, (255, 40, 40));
            assert_eq!(end, (255, 20, 147));
        });
    }

    #[test]
    fn footer_fade_is_light_blue_to_dark_cyan() {
        with_theme(UiTheme::Pulse, || {
            let start = match fade_color_with(FadePalette::Footer, 0.0) {
                Color::Rgb(r, g, b) => (r, g, b),
                other => panic!("{other:?}"),
            };
            let end = match fade_color_with(FadePalette::Footer, 1.0) {
                Color::Rgb(r, g, b) => (r, g, b),
                other => panic!("{other:?}"),
            };
            assert_eq!(start, (135, 206, 250));
            assert_eq!(end, (0, 139, 139));
        });
    }

    #[test]
    fn cycle_theme_changes_box_not_banner_or_address() {
        with_theme(UiTheme::Pulse, || {
            let banner_before = fade_color_with(FadePalette::Banner, 0.0);
            let addr = colored_address_spans("0x1234567890abcdef1234567890abcdef12345678");
            let addr_green = addr[1].style.fg;

            set_theme(UiTheme::Ocean);
            assert_ne!(
                fade_color_with(FadePalette::Box, 0.0),
                Color::Rgb(255, 40, 40)
            );
            assert_eq!(fade_color_with(FadePalette::Banner, 0.0), banner_before);
            assert_eq!(
                colored_address_spans("0x1234567890abcdef1234567890abcdef12345678")[1]
                    .style
                    .fg,
                addr_green
            );
        });
    }

    #[test]
    fn solid_theme_has_no_border_fade() {
        with_theme(UiTheme::Slate, || {
            assert!(!UiTheme::Slate.fades());
            let a = fade_color_with(FadePalette::Box, 0.0);
            let b = fade_color_with(FadePalette::Box, 1.0);
            assert_eq!(a, b);
            assert_eq!(accent_color(), Color::Rgb(200, 205, 215));
            assert_ne!(fade_color_with(FadePalette::Banner, 0.0), a);
        });
    }

    #[test]
    fn phosphor_changes_text_ink() {
        with_theme(UiTheme::Phosphor, || {
            assert_eq!(accent_color(), Color::Rgb(0, 255, 70));
            assert_eq!(body_color(), Color::Rgb(0, 180, 50));
            let title = fade_line(" Hi ");
            assert!(title
                .spans
                .iter()
                .all(|s| s.style.fg == Some(title_color())));
        });
    }

    #[test]
    fn theme_catalog_matches_enum() {
        assert_eq!(THEME_CATALOG.len(), UiTheme::ALL.len());
        for (i, theme) in UiTheme::ALL.iter().enumerate() {
            assert_eq!(*theme as usize, i);
            assert_eq!(theme.spec().id, THEME_CATALOG[i].id);
        }
    }

    #[test]
    fn theme_id_roundtrip() {
        for theme in UiTheme::ALL {
            assert_eq!(UiTheme::parse(theme.id()), Some(theme));
            assert_eq!(
                UiTheme::parse(&theme.id().to_ascii_uppercase()),
                Some(theme)
            );
        }
        assert_eq!(UiTheme::parse("nope"), None);
    }

    #[test]
    fn persist_theme_roundtrip_via_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui-theme");
        std::fs::write(&path, UiTheme::Violet.id()).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(UiTheme::parse(&raw), Some(UiTheme::Violet));
    }

    #[test]
    fn fade_line_matches_length() {
        let line = fade_line("Hello");
        let s: String = line.spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert_eq!(s, "Hello");
        assert_eq!(line.spans.len(), 5);
    }

    #[test]
    fn faded_box_paints_square_corners_and_inner() {
        let area = Rect::new(0, 0, 12, 5);
        let mut buf = Buffer::empty(area);
        let inner = render_faded_box_buf(&mut buf, area, Some(fade_line(" Hi ")));
        assert_eq!(inner, Rect::new(1, 1, 10, 3));
        assert_eq!(buf[(0, 0)].symbol(), "┌");
        assert_eq!(buf[(11, 0)].symbol(), "┐");
        assert_eq!(buf[(0, 4)].symbol(), "└");
        assert_eq!(buf[(11, 4)].symbol(), "┘");
        // Title starts two cells in.
        assert_eq!(buf[(2, 0)].symbol(), " ");
        assert_eq!(buf[(3, 0)].symbol(), "H");
    }

    #[test]
    fn colored_address_segments_match_dioxus() {
        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        let spans = colored_address_spans(addr);
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, addr);
        assert_eq!(spans[0].content.as_ref(), "0x");
        assert_eq!(spans[0].style.fg, Some(ADDR_GREY));
        assert_eq!(spans[1].content.as_ref(), "12345");
        assert_eq!(spans[1].style.fg, Some(ADDR_GREEN));
        assert_eq!(spans[2].content.as_ref(), "67890abcdef12");
        assert_eq!(spans[2].style.fg, Some(ADDR_GREY));
        assert_eq!(spans[3].content.as_ref(), "34567");
        assert_eq!(spans[3].style.fg, Some(ADDR_ORANGE));
        assert_eq!(spans[4].content.as_ref(), "890abcdef123");
        assert_eq!(spans[4].style.fg, Some(ADDR_GREY));
        assert_eq!(spans[5].content.as_ref(), "45678");
        assert_eq!(spans[5].style.fg, Some(ADDR_PURPLE));
    }

    #[test]
    fn colored_address_fitted_preserves_budget() {
        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        let spans = colored_address_fitted(addr, 14);
        let s: String = spans.iter().map(|sp| sp.content.as_ref()).collect();
        assert!(s.contains('…'), "{s}");
        assert!(s.chars().count() <= 14, "{s}");
    }

    #[test]
    fn colored_address_short_is_grey() {
        let spans = colored_address_spans("(locked)");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style.fg, Some(ADDR_GREY));
    }

    #[test]
    fn orange_segment_aligns_under_augha() {
        let width = 80u16;
        let banner = logo_banner(width);
        let banner_s: String = banner.spans.iter().map(|s| s.content.as_ref()).collect();
        let vaughan_at = banner_s.find("VAUGHAN").expect("wordmark");
        let augha_at = vaughan_at + 1;
        assert_eq!(wordmark_augha_start(width), augha_at);
        assert_eq!(&banner_s[augha_at..augha_at + 5], "AUGHA");

        let addr = "0x1234567890abcdef1234567890abcdef12345678";
        let line = colored_address_under_augha(addr, width);

        // Walk columns until we hit the orange-styled span (body[18..23]).
        let mut col = 0usize;
        let mut orange_at = None;
        let mut orange_text = String::new();
        for span in &line.spans {
            let len = span.content.chars().count();
            if span.style.fg == Some(ADDR_ORANGE) {
                orange_at = Some(col);
                orange_text = span.content.to_string();
                break;
            }
            col += len;
        }
        let orange_at = orange_at.expect("orange span");
        assert_eq!(orange_text.len(), 5);
        assert_eq!(
            orange_at, augha_at,
            "orange should start under AUGHA\nbanner: {banner_s}\ncol={orange_at} augha={augha_at}"
        );
    }
}
