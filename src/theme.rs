//! Colors by role, in a flat, Material-style scheme: a base `background`, `surface`s one
//! or more tones up (cards, then raised panels), and text and accent colors meant to be read
//! on them. Built-in themes, custom ones from the config (from a base theme, a seed color,
//! and per-role overrides), live switching, and a 256-color fallback.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use ratatui::style::{Color, Style};
use serde::Deserialize;

/// Declares `Theme` with one `Color` field per role, plus `ROLES` (name and description)
/// and by-name access for the config and the editor.
macro_rules! roles {
    ($($name:ident: $doc:literal,)*) => {
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct Theme {
            $(#[doc = $doc] pub $name: Color,)*
        }

        /// Every role: config name and what it colors.
        pub const ROLES: &[(&str, &str)] = &[$((stringify!($name), $doc),)*];

        impl Theme {
            pub fn get(&self, role: &str) -> Option<Color> {
                match role {
                    $(stringify!($name) => Some(self.$name),)*
                    _ => None,
                }
            }

            fn set(&mut self, role: &str, color: Color) -> bool {
                match role {
                    $(stringify!($name) => self.$name = color,)*
                    _ => return false,
                }
                true
            }
        }
    };
}

roles! {
    background: "Base of the screen, and the gaps between cards",
    surface: "Cards: catalog entries and posts",
    surface_high: "Chips and tiles on cards",
    surface_highest: "Raised panels: help, quote preview, settings pickers",
    bar: "The top and bottom bars",
    on_bar: "Text on the bars",
    text: "Body text",
    text_dim: "Secondary text: times, numbers, hints",
    primary: "Accent: titles, keys, the selection stripe",
    on_primary: "Text on the accent color",
    primary_container: "Emphasis containers: OP and title chips",
    on_primary_container: "Text on emphasis containers",
    selection: "Background of the selected row, card or post",
    greentext: "Greentext (>quotes)",
    pinktext: "Pinktext (<quotes) and orange text",
    quotelink: "Quote links (>>123)",
    heading: "Red text and headings in posts",
    code: "Code text",
    code_bg: "Background of code blocks",
    name: "Poster names",
    new: "New posts and unread counts",
    error: "Errors",
    success: "Confirmations",
    warning: "Warnings",
    search: "Background of search matches",
    on_search: "Text of search matches",
    spoiler: "Hidden spoilers (text and background)",
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[rustfmt::skip]
const MATERIAL: Theme = Theme {
    background: rgb(0x141218), surface: rgb(0x1d1b20), surface_high: rgb(0x2b2930), surface_highest: rgb(0x36343b),
    bar: rgb(0x211f26), on_bar: rgb(0xe6e0e9), text: rgb(0xe6e0e9), text_dim: rgb(0x938f99),
    primary: rgb(0xd0bcff), on_primary: rgb(0x381e72), primary_container: rgb(0x4f378b), on_primary_container: rgb(0xeaddff),
    selection: rgb(0x3b3549), greentext: rgb(0xa6d49b), pinktext: rgb(0xffb4ab), quotelink: rgb(0xefb8c8),
    heading: rgb(0xf2b8b5), code: rgb(0xb0c6ff), code_bg: rgb(0x0f0d13), name: rgb(0x9cd67d), new: rgb(0x9fe0a8),
    error: rgb(0xf2b8b5), success: rgb(0x9fe0a8), warning: rgb(0xe8c46e), search: rgb(0xe8c46e), on_search: rgb(0x1d1b20),
    spoiler: rgb(0x49454f),
};

#[rustfmt::skip]
const MATERIAL_LIGHT: Theme = Theme {
    background: rgb(0xfef7ff), surface: rgb(0xf3edf7), surface_high: rgb(0xe6e0e9), surface_highest: rgb(0xece6f0),
    bar: rgb(0xece6f0), on_bar: rgb(0x1d1b20), text: rgb(0x1d1b20), text_dim: rgb(0x79747e),
    primary: rgb(0x6750a4), on_primary: rgb(0xffffff), primary_container: rgb(0xeaddff), on_primary_container: rgb(0x21005d),
    selection: rgb(0xe3d7f7), greentext: rgb(0x386a20), pinktext: rgb(0xa0446e), quotelink: rgb(0x7d5260),
    heading: rgb(0xb3261e), code: rgb(0x34518f), code_bg: rgb(0xfaf5fc), name: rgb(0x2e6b30), new: rgb(0x2e7d32),
    error: rgb(0xb3261e), success: rgb(0x2e7d32), warning: rgb(0x8b5000), search: rgb(0xffdf99), on_search: rgb(0x251a00),
    spoiler: rgb(0xcac4d0),
};

#[rustfmt::skip]
const NORD: Theme = Theme {
    background: rgb(0x2e3440), surface: rgb(0x3b4252), surface_high: rgb(0x4c566a), surface_highest: rgb(0x434c5e),
    bar: rgb(0x272c36), on_bar: rgb(0xd8dee9), text: rgb(0xeceff4), text_dim: rgb(0x8e99ae),
    primary: rgb(0x88c0d0), on_primary: rgb(0x2e3440), primary_container: rgb(0x5e81ac), on_primary_container: rgb(0xeceff4),
    selection: rgb(0x4a5568), greentext: rgb(0xa3be8c), pinktext: rgb(0xd08770), quotelink: rgb(0xb48ead),
    heading: rgb(0xbf616a), code: rgb(0x8fbcbb), code_bg: rgb(0x2a2f3a), name: rgb(0xa3be8c), new: rgb(0xa3be8c),
    error: rgb(0xbf616a), success: rgb(0xa3be8c), warning: rgb(0xebcb8b), search: rgb(0xebcb8b), on_search: rgb(0x2e3440),
    spoiler: rgb(0x4c566a),
};

#[rustfmt::skip]
const GRUVBOX: Theme = Theme {
    background: rgb(0x282828), surface: rgb(0x32302f), surface_high: rgb(0x504945), surface_highest: rgb(0x3c3836),
    bar: rgb(0x1d2021), on_bar: rgb(0xebdbb2), text: rgb(0xebdbb2), text_dim: rgb(0xa89984),
    primary: rgb(0xfabd2f), on_primary: rgb(0x282828), primary_container: rgb(0x7c5b16), on_primary_container: rgb(0xfbf1c7),
    selection: rgb(0x45403d), greentext: rgb(0xb8bb26), pinktext: rgb(0xfe8019), quotelink: rgb(0xd3869b),
    heading: rgb(0xfb4934), code: rgb(0x83a598), code_bg: rgb(0x1d2021), name: rgb(0x8ec07c), new: rgb(0xb8bb26),
    error: rgb(0xfb4934), success: rgb(0xb8bb26), warning: rgb(0xfe8019), search: rgb(0xfabd2f), on_search: rgb(0x282828),
    spoiler: rgb(0x504945),
};

#[rustfmt::skip]
const CATPPUCCIN: Theme = Theme {
    background: rgb(0x1e1e2e), surface: rgb(0x262637), surface_high: rgb(0x45475a), surface_highest: rgb(0x313244),
    bar: rgb(0x181825), on_bar: rgb(0xcdd6f4), text: rgb(0xcdd6f4), text_dim: rgb(0x9399b2),
    primary: rgb(0xcba6f7), on_primary: rgb(0x1e1e2e), primary_container: rgb(0x4e3d73), on_primary_container: rgb(0xebddff),
    selection: rgb(0x38364f), greentext: rgb(0xa6e3a1), pinktext: rgb(0xfab387), quotelink: rgb(0xf5c2e7),
    heading: rgb(0xf38ba8), code: rgb(0x89dceb), code_bg: rgb(0x11111b), name: rgb(0xa6e3a1), new: rgb(0xa6e3a1),
    error: rgb(0xf38ba8), success: rgb(0xa6e3a1), warning: rgb(0xf9e2af), search: rgb(0xf9e2af), on_search: rgb(0x1e1e2e),
    spoiler: rgb(0x45475a),
};

#[rustfmt::skip]
const TOKYO_NIGHT: Theme = Theme {
    background: rgb(0x1a1b26), surface: rgb(0x1f2335), surface_high: rgb(0x3b4261), surface_highest: rgb(0x292e42),
    bar: rgb(0x16161e), on_bar: rgb(0xc0caf5), text: rgb(0xc0caf5), text_dim: rgb(0x737aa2),
    primary: rgb(0x7aa2f7), on_primary: rgb(0x1a1b26), primary_container: rgb(0x3d59a1), on_primary_container: rgb(0xc0caf5),
    selection: rgb(0x283457), greentext: rgb(0x9ece6a), pinktext: rgb(0xff9e64), quotelink: rgb(0xbb9af7),
    heading: rgb(0xf7768e), code: rgb(0x7dcfff), code_bg: rgb(0x16161e), name: rgb(0x9ece6a), new: rgb(0x9ece6a),
    error: rgb(0xf7768e), success: rgb(0x9ece6a), warning: rgb(0xe0af68), search: rgb(0xe0af68), on_search: rgb(0x1a1b26),
    spoiler: rgb(0x3b4261),
};

#[rustfmt::skip]
const SOLARIZED_LIGHT: Theme = Theme {
    background: rgb(0xfdf6e3), surface: rgb(0xf3ecd8), surface_high: rgb(0xe4ddc8), surface_highest: rgb(0xeee8d5),
    bar: rgb(0xeee8d5), on_bar: rgb(0x475b62), text: rgb(0x3e5158), text_dim: rgb(0x8a9a9b),
    primary: rgb(0x268bd2), on_primary: rgb(0xfdf6e3), primary_container: rgb(0xd5e6f2), on_primary_container: rgb(0x073642),
    selection: rgb(0xdfe7ea), greentext: rgb(0x718200), pinktext: rgb(0xcb4b16), quotelink: rgb(0x6c71c4),
    heading: rgb(0xdc322f), code: rgb(0x2aa198), code_bg: rgb(0xf8f2df), name: rgb(0x718200), new: rgb(0x718200),
    error: rgb(0xdc322f), success: rgb(0x718200), warning: rgb(0xb58900), search: rgb(0xf0d78c), on_search: rgb(0x073642),
    spoiler: rgb(0x93a1a1),
};

/// The terminal's own colors: its background stays (transparency works), with the 16 ANSI
/// colors for accents. For terminals without 256 colors, or for a native look.
#[rustfmt::skip]
const TERMINAL: Theme = Theme {
    background: Color::Reset, surface: Color::Reset, surface_high: Color::Black, surface_highest: Color::Black,
    bar: Color::Black, on_bar: Color::Reset, text: Color::Reset, text_dim: Color::DarkGray,
    primary: Color::Yellow, on_primary: Color::Black, primary_container: Color::Blue, on_primary_container: Color::White,
    selection: Color::Blue, greentext: Color::Green, pinktext: Color::LightRed, quotelink: Color::Magenta,
    heading: Color::Red, code: Color::Cyan, code_bg: Color::Reset, name: Color::Green, new: Color::LightGreen,
    error: Color::Red, success: Color::Green, warning: Color::Yellow, search: Color::Yellow, on_search: Color::Black,
    spoiler: Color::DarkGray,
};

/// No colors at all, only the terminal's own (for `NO_COLOR`): what's selected, focused or
/// a spoiler shows by glyphs and bold or underlined text instead.
#[rustfmt::skip]
const MONO: Theme = Theme {
    background: Color::Reset, surface: Color::Reset, surface_high: Color::Reset, surface_highest: Color::Reset,
    bar: Color::Reset, on_bar: Color::Reset, text: Color::Reset, text_dim: Color::Reset,
    primary: Color::Reset, on_primary: Color::Reset, primary_container: Color::Reset, on_primary_container: Color::Reset,
    selection: Color::Reset, greentext: Color::Reset, pinktext: Color::Reset, quotelink: Color::Reset,
    heading: Color::Reset, code: Color::Reset, code_bg: Color::Reset, name: Color::Reset, new: Color::Reset,
    error: Color::Reset, success: Color::Reset, warning: Color::Reset, search: Color::Reset, on_search: Color::Reset,
    spoiler: Color::Reset,
};

pub const DEFAULT_THEME: &str = "material";

/// The theme when the config names none: "mono" if `NO_COLOR` is set (and not empty, see
/// no-color.org), else `DEFAULT_THEME`.
pub fn default_name() -> &'static str {
    default_for(std::env::var_os("NO_COLOR").as_deref())
}

pub(crate) fn default_for(no_color: Option<&std::ffi::OsStr>) -> &'static str {
    if no_color.is_some_and(|v| !v.is_empty()) { "mono" } else { DEFAULT_THEME }
}

/// Built-in themes, in the order the picker lists them.
pub const BUILTIN: &[(&str, Theme)] = &[
    ("material", MATERIAL),
    ("material-light", MATERIAL_LIGHT),
    ("nord", NORD),
    ("gruvbox", GRUVBOX),
    ("catppuccin", CATPPUCCIN),
    ("tokyo-night", TOKYO_NIGHT),
    ("solarized-light", SOLARIZED_LIGHT),
    ("terminal", TERMINAL),
    ("mono", MONO),
];

/// A custom theme from `[themes.NAME]`: start from `base` (a built-in or another custom
/// theme) or generate from `seed`, then override any role.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ThemeDef {
    pub base: Option<String>,
    /// A color to derive the whole scheme from, Material-style.
    pub seed: Option<String>,
    /// For `seed`: "dark" (default) or "light".
    pub mode: Option<String>,
    /// Role overrides: `primary = "#88c0d0"`.
    #[serde(flatten)]
    pub colors: BTreeMap<String, String>,
}

/// The config's `theme`: a theme name, or (ck 0.2) a table of color overrides.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ThemeSetting {
    Name(String),
    Legacy(BTreeMap<String, String>),
}

/// ck 0.2's `[theme]` keys, and the roles they now mean.
pub const LEGACY_KEYS: &[(&str, &str)] = &[
    ("accent", "primary"),
    ("dim", "text_dim"),
    ("selected", "selection"),
    ("search", "search"),
    ("name", "name"),
    ("greentext", "greentext"),
    ("quotelink", "quotelink"),
    ("heading", "heading"),
    ("code", "code"),
    ("new", "new"),
];

/// Parse a color: a name ("light-blue"), "#rrggbb", a 256-color index ("244"), or "default"
/// for the terminal's own color.
pub fn parse_color(s: &str) -> Result<Color> {
    match s.trim().to_ascii_lowercase().as_str() {
        "default" | "reset" | "none" => Ok(Color::Reset),
        t => Color::from_str(t).map_err(|_| anyhow!("\"{s}\" isn't a color (try #rrggbb, a name, or 0-255)")),
    }
}

/// A color as the config writes it.
pub fn color_string(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => i.to_string(),
        Color::Reset => "default".into(),
        named => format!("{named}").to_lowercase().replace(' ', "-"),
    }
}

/// Every theme name: built-ins, then custom ones.
pub fn names(custom: &BTreeMap<String, ThemeDef>) -> Vec<String> {
    let mut out: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
    out.extend(custom.keys().filter(|k| !BUILTIN.iter().any(|(n, _)| n == k)).cloned());
    out
}

/// Resolve a theme by name. Custom themes may build on each other (up to a few levels).
pub fn resolve(name: &str, custom: &BTreeMap<String, ThemeDef>) -> Result<Theme> {
    resolve_depth(name, custom, 0)
}

fn resolve_depth(name: &str, custom: &BTreeMap<String, ThemeDef>, depth: usize) -> Result<Theme> {
    if depth > 8 {
        bail!("[themes.{name}]: themes are based on each other in a loop");
    }
    let Some(def) = custom.get(name) else {
        return BUILTIN
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| *t)
            .ok_or_else(|| anyhow!("unknown theme \"{name}\" (built-in: {})", names(&BTreeMap::new()).join(", ")));
    };
    let mut theme = match (&def.seed, &def.base) {
        (Some(seed), _) => {
            let light = match def.mode.as_deref().unwrap_or("dark") {
                "dark" => false,
                "light" => true,
                m => bail!("[themes.{name}]: mode = \"{m}\" should be \"dark\" or \"light\""),
            };
            from_seed(parse_color(seed).map_err(|e| anyhow!("[themes.{name}] seed: {e}"))?, light)
        }
        // A custom theme named like a built-in starts from that built-in.
        (None, Some(base)) if base != name => resolve_depth(base, custom, depth + 1)?,
        _ => BUILTIN.iter().find(|(n, _)| *n == name).map_or(MATERIAL, |(_, t)| *t),
    };
    apply(&mut theme, &def.colors).map_err(|e| anyhow!("[themes.{name}] {e}"))?;
    Ok(theme)
}

/// Apply role overrides, rejecting unknown roles and bad colors.
pub fn apply(theme: &mut Theme, colors: &BTreeMap<String, String>) -> Result<()> {
    for (role, value) in colors {
        let color = parse_color(value).map_err(|e| anyhow!("{role}: {e}"))?;
        if !theme.set(role, color) {
            let known: Vec<_> = ROLES.iter().map(|(n, _)| *n).collect();
            bail!("unknown color role `{role}` (roles: {})", known.join(", "));
        }
    }
    Ok(())
}

/// The theme the config asks for: `theme = "name"`, or ck 0.2's `[theme]` overrides on the
/// default theme.
pub fn from_config(setting: Option<&ThemeSetting>, custom: &BTreeMap<String, ThemeDef>) -> Result<Theme> {
    match setting {
        None => resolve(default_name(), custom),
        Some(ThemeSetting::Name(n)) => resolve(n, custom),
        Some(ThemeSetting::Legacy(old)) => {
            let mut theme = resolve(DEFAULT_THEME, custom)?;
            let mut colors = BTreeMap::new();
            for (key, value) in old {
                let role = LEGACY_KEYS.iter().find(|(k, _)| k == key).map(|(_, r)| *r).unwrap_or(key);
                colors.insert(role.to_string(), value.clone());
            }
            apply(&mut theme, &colors).map_err(|e| anyhow!("[theme] {e}"))?;
            Ok(theme)
        }
    }
}

// ----- generating a scheme from one color (OKLCH tones) -----

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// sRGB to OKLCH: lightness 0-1, chroma, hue in degrees.
fn to_oklch(c: Color) -> (f32, f32, f32) {
    let Color::Rgb(r, g, b) = c else { return (0.6, 0.1, 280.0) };
    let [r, g, b] = [r, g, b].map(|v| srgb_to_linear(v as f32 / 255.0));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let lab_l = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
    let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let bb = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    (lab_l, a.hypot(bb), bb.atan2(a).to_degrees().rem_euclid(360.0))
}

/// OKLCH to sRGB, lowering chroma until the color fits in sRGB.
fn oklch(l: f32, c: f32, h: f32) -> Color {
    let mut c = c;
    loop {
        let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
        let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
        let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
        let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
        let rgb = [
            4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
            -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
            -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
        ];
        if rgb.iter().all(|v| (-0.001..=1.001).contains(v)) || c < 0.002 {
            let [r, g, b] = rgb.map(|v| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
            return Color::Rgb(r, g, b);
        }
        c *= 0.9;
    }
}

/// A full scheme from one color: neutral surfaces tinted with its hue, the accent at a
/// readable tone, and fixed hues for greentext, links and status colors.
pub fn from_seed(seed: Color, light: bool) -> Theme {
    let (_, chroma, hue) = to_oklch(seed);
    let accent_c = chroma.clamp(0.06, 0.14);
    // Tone for dark, light: neutrals get a hint of the seed's hue.
    let t = |dark: f32, lite: f32| if light { lite } else { dark };
    let n = |dark: f32, lite: f32| oklch(t(dark, lite), 0.012, hue);
    let tinted = |dark: f32, lite: f32, c: f32| oklch(t(dark, lite), c, hue);
    let fixed = |h: f32| oklch(t(0.82, 0.48), 0.12, h);
    Theme {
        background: n(0.17, 0.985),
        surface: n(0.21, 0.955),
        surface_high: n(0.31, 0.89),
        surface_highest: n(0.26, 0.925),
        bar: n(0.19, 0.93),
        on_bar: n(0.92, 0.22),
        text: n(0.93, 0.2),
        text_dim: n(0.68, 0.52),
        primary: tinted(0.82, 0.48, accent_c),
        on_primary: tinted(0.24, 0.99, accent_c * 0.5),
        primary_container: tinted(0.38, 0.9, accent_c * 0.7),
        on_primary_container: tinted(0.93, 0.25, accent_c * 0.4),
        selection: tinted(0.29, 0.9, 0.04),
        greentext: fixed(140.0),
        pinktext: fixed(40.0),
        quotelink: oklch(t(0.8, 0.5), 0.1, (hue + 60.0) % 360.0),
        heading: oklch(t(0.75, 0.52), 0.15, 25.0),
        code: fixed(230.0),
        code_bg: n(0.14, 0.97),
        name: fixed(145.0),
        new: oklch(t(0.84, 0.5), 0.14, 145.0),
        error: oklch(t(0.75, 0.52), 0.15, 25.0),
        success: oklch(t(0.84, 0.5), 0.14, 145.0),
        warning: oklch(t(0.85, 0.6), 0.13, 80.0),
        search: oklch(t(0.85, 0.9), 0.13, 90.0),
        on_search: n(0.2, 0.2),
        spoiler: n(0.36, 0.82),
    }
}

// ----- the current theme -----

thread_local! {
    /// Only the UI thread draws, so the current theme is per thread (which also keeps tests,
    /// each on its own thread, independent).
    static CURRENT: Cell<Theme> = const { Cell::new(MATERIAL) };
}

pub fn theme() -> Theme {
    CURRENT.get()
}

pub fn set(theme: Theme) {
    CURRENT.set(theme);
}

/// Colors that parsed posts carry instead of real ones, so their look follows the current
/// theme (they're mapped by `paint` when drawn). Values no theme would use.
pub mod mark {
    use ratatui::style::Color;

    pub const GREENTEXT: Color = Color::Rgb(1, 2, 1);
    pub const PINKTEXT: Color = Color::Rgb(1, 2, 2);
    pub const QUOTELINK: Color = Color::Rgb(1, 2, 3);
    pub const HEADING: Color = Color::Rgb(1, 2, 4);
    pub const CODE: Color = Color::Rgb(1, 2, 5);
    pub const SPOILER: Color = Color::Rgb(1, 2, 6);
    /// Text of a revealed spoiler.
    pub const REVEALED: Color = Color::Rgb(1, 2, 7);
    /// A web link in a post.
    pub const LINK: Color = Color::Rgb(1, 2, 8);
}

fn paint_color(c: Option<Color>, t: &Theme) -> Option<Color> {
    Some(match c? {
        mark::GREENTEXT => t.greentext,
        mark::PINKTEXT => t.pinktext,
        mark::QUOTELINK => t.quotelink,
        mark::HEADING => t.heading,
        mark::CODE => t.code,
        mark::SPOILER => t.spoiler,
        mark::REVEALED => t.text,
        mark::LINK => t.quotelink,
        other => other,
    })
}

/// A style with post markup colors replaced by the current theme's.
pub fn paint(style: Style) -> Style {
    let t = theme();
    Style { fg: paint_color(style.fg, &t), bg: paint_color(style.bg, &t), ..style }
}

// ----- color depth -----

/// The nearest xterm 256-color index, for terminals without 24-bit color.
pub fn to_256(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else { return c };
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| LEVELS.iter().enumerate().min_by_key(|(_, l)| (**l as i32 - v as i32).abs()).map_or(0, |(i, _)| i);
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    // The 24 grays, 8..238.
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gray_i = ((avg.saturating_sub(3)) / 10).min(23) as u8;
    let gray = 8 + gray_i * 10;
    let dist = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    if dist((gray, gray, gray)) < dist(cube) {
        Color::Indexed(232 + gray_i)
    } else {
        Color::Indexed(16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8)
    }
}

/// Whether the terminal says it shows 24-bit color.
pub fn truecolor_terminal() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "truecolor" | "24bit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_and_print() {
        assert_eq!(parse_color("#ff8800").unwrap(), Color::Rgb(255, 136, 0));
        assert_eq!(parse_color("light-green").unwrap(), Color::LightGreen);
        assert_eq!(parse_color("244").unwrap(), Color::Indexed(244));
        assert_eq!(parse_color("default").unwrap(), Color::Reset);
        assert!(parse_color("blurple").is_err());
        for c in [Color::Rgb(1, 2, 3), Color::Indexed(7), Color::Reset, Color::LightGreen] {
            assert_eq!(parse_color(&color_string(c)).unwrap(), c);
        }
    }

    #[test]
    fn builtins_and_custom_themes() {
        let mut custom = BTreeMap::new();
        assert_eq!(resolve("nord", &custom).unwrap(), NORD);
        assert!(resolve("nope", &custom).unwrap_err().to_string().contains("unknown theme"));
        // Based on another theme, with an override.
        let def = |base: &str, colors: &[(&str, &str)]| ThemeDef {
            base: Some(base.into()),
            colors: colors.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Default::default()
        };
        custom.insert("mine".into(), def("gruvbox", &[("primary", "#123456")]));
        custom.insert("mine2".into(), def("mine", &[("text", "white")]));
        let t = resolve("mine2", &custom).unwrap();
        assert_eq!((t.primary, t.text, t.background), (Color::Rgb(0x12, 0x34, 0x56), Color::White, GRUVBOX.background));
        // Loops and bad roles are errors.
        custom.insert("a".into(), def("b", &[]));
        custom.insert("b".into(), def("a", &[]));
        assert!(resolve("a", &custom).unwrap_err().to_string().contains("loop"));
        custom.insert("bad".into(), def("nord", &[("prmary", "red")]));
        assert!(resolve("bad", &custom).unwrap_err().to_string().contains("unknown color role `prmary`"));
        assert_eq!(names(&custom).first().map(String::as_str), Some("material"));
    }

    #[test]
    fn seed_themes_have_contrast() {
        for light in [false, true] {
            let t = from_seed(Color::Rgb(0x88, 0xc0, 0xd0), light);
            let lum = |c: Color| match c {
                Color::Rgb(r, g, b) => 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32,
                _ => 0.0,
            };
            // Text stands well apart from the background; surfaces step away from it.
            assert!((lum(t.text) - lum(t.background)).abs() > 150.0, "light={light}");
            assert_ne!(t.surface, t.background);
            assert!((lum(t.surface) - lum(t.background)).abs() < 40.0);
        }
    }

    #[test]
    fn legacy_theme_table_still_works() {
        let old: BTreeMap<String, String> = [("accent".to_string(), "cyan".to_string())].into();
        let t = from_config(Some(&ThemeSetting::Legacy(old)), &BTreeMap::new()).unwrap();
        assert_eq!(t.primary, Color::Cyan);
    }

    #[test]
    fn marks_follow_the_theme() {
        set(NORD);
        assert_eq!(paint(Style::new().fg(mark::GREENTEXT)).fg, Some(NORD.greentext));
        set(GRUVBOX);
        assert_eq!(paint(Style::new().fg(mark::GREENTEXT).bg(mark::SPOILER)).bg, Some(GRUVBOX.spoiler));
        assert_eq!(paint(Style::new().fg(Color::Red)).fg, Some(Color::Red));
    }

    #[test]
    fn fallback_to_256_colors() {
        assert_eq!(to_256(Color::Rgb(0, 0, 0)), Color::Indexed(16));
        assert_eq!(to_256(Color::Rgb(255, 255, 255)), Color::Indexed(231));
        assert_eq!(to_256(Color::Rgb(0x30, 0x30, 0x30)), Color::Indexed(236));
        assert_eq!(to_256(Color::Red), Color::Red);
    }
}
