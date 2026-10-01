//! Colors, configurable in the `[theme]` section of config.toml.

use std::str::FromStr;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use ratatui::style::Color;
use serde::Deserialize;

/// `[theme]` as written in the config: color names (`"yellow"`, `"light-red"`), `"#rrggbb"`,
/// or 256-color indexes (`"244"`). Everything is optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeConfig {
    accent: Option<String>,
    dim: Option<String>,
    selected: Option<String>,
    search: Option<String>,
    name: Option<String>,
    greentext: Option<String>,
    quotelink: Option<String>,
    heading: Option<String>,
    code: Option<String>,
    new: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Theme {
    /// Titles, the header badge, subjects, the selection gutter.
    pub accent: Color,
    /// Secondary text: times, hints, placeholders.
    pub dim: Color,
    /// Background of the selected list row.
    pub selected: Color,
    /// Background of search matches.
    pub search: Color,
    /// Poster names.
    pub name: Color,
    pub greentext: Color,
    pub quotelink: Color,
    pub heading: Color,
    pub code: Color,
    /// The "● new" marker and unread counts.
    pub new: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Yellow,
            dim: Color::DarkGray,
            selected: Color::Rgb(45, 45, 60),
            search: Color::Yellow,
            name: Color::Green,
            greentext: Color::Green,
            quotelink: Color::Magenta,
            heading: Color::Red,
            code: Color::Cyan,
            new: Color::Green,
        }
    }
}

static THEME: OnceLock<Theme> = OnceLock::new();

impl Theme {
    pub fn from_config(c: &ThemeConfig) -> Result<Self> {
        let d = Theme::default();
        let color = |field: &str, v: &Option<String>, default: Color| -> Result<Color> {
            match v {
                None => Ok(default),
                Some(s) => Color::from_str(s).map_err(|_| anyhow!("[theme]: {field} = \"{s}\" isn't a color")),
            }
        };
        Ok(Self {
            accent: color("accent", &c.accent, d.accent)?,
            dim: color("dim", &c.dim, d.dim)?,
            selected: color("selected", &c.selected, d.selected)?,
            search: color("search", &c.search, d.search)?,
            name: color("name", &c.name, d.name)?,
            greentext: color("greentext", &c.greentext, d.greentext)?,
            quotelink: color("quotelink", &c.quotelink, d.quotelink)?,
            heading: color("heading", &c.heading, d.heading)?,
            code: color("code", &c.code, d.code)?,
            new: color("new", &c.new, d.new)?,
        })
    }
}

/// Set the theme once at startup, before anything is parsed or drawn.
pub fn init(theme: Theme) {
    let _ = THEME.set(theme);
}

pub fn theme() -> &'static Theme {
    THEME.get_or_init(Theme::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors_and_rejects_bad_ones() {
        let c: ThemeConfig = toml::from_str("accent = \"#ff8800\"\ngreentext = \"light-green\"\ndim = \"244\"").unwrap();
        let t = Theme::from_config(&c).unwrap();
        assert_eq!(t.accent, Color::Rgb(255, 136, 0));
        assert_eq!(t.greentext, Color::LightGreen);
        assert_eq!(t.dim, Color::Indexed(244));
        assert_eq!(t.quotelink, Theme::default().quotelink);

        let c: ThemeConfig = toml::from_str("accent = \"blurple\"").unwrap();
        assert!(Theme::from_config(&c).unwrap_err().to_string().contains("accent"));
        assert!(toml::from_str::<ThemeConfig>("acent = \"red\"").is_err());
    }
}
