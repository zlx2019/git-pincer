//! UI themes: centralized definitions for all colors.
//!
//! The dark default is Tokyo Night, tuned for dark terminals; light uses Maple Light.
//! Selection increases contrast and saturation within the same hue to avoid muddy colors.
//! Users can override named colors in `[theme.dark]` / `[theme.light]` configuration;
//! overrides are applied when [`Theme::select`] builds a theme.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{Result, bail};
use ratatui::style::Color;

use crate::config::{ColorValue, ThemeSection, parse_hex};
use crate::i18n::tr_f;

use super::rows::ChangeType;

/// All overridable colors: (name, whether the value is a (normal, selected) pair).
const COLOR_FIELDS: &[(&str, bool)] = &[
    ("fg_dim", false),
    ("fg_bright", false),
    ("blue", false),
    ("green", false),
    ("red", false),
    ("amber", false),
    ("border", false),
    ("badge_fg", false),
    ("scrollbar_thumb", false),
    ("keycap_fg", false),
    ("keycap_bg", false),
    ("hint_fg", false),
    ("logo", false),
    ("placeholder_fg", false),
    ("rpg_frame", false),
    ("rpg_accent", false),
    ("rpg_hp", false),
    ("rpg_mp", false),
    ("rpg_exp", false),
    ("rpg_gauge_empty", false),
    ("rpg_gold", false),
    ("band_modified", true),
    ("band_added", true),
    ("band_deleted", true),
    ("band_conflict", true),
    ("emph_modified", true),
    ("emph_added", true),
    ("emph_deleted", true),
    ("emph_conflict", true),
];

/// One parsed color override.
#[derive(Debug, Clone, Copy)]
enum OverrideValue {
    /// Single color
    Single((u8, u8, u8)),
    /// (Normal, selected) color pair
    Pair((u8, u8, u8), (u8, u8, u8)),
}

/// Parsed override maps for both dark and light themes.
type Overrides = HashMap<String, OverrideValue>;

/// Process-wide theme overrides, frozen by init; empty before initialization.
static OVERRIDES: OnceLock<(Overrides, Overrides)> = OnceLock::new();

/// Validate and apply `[theme]` overrides before constructing any [`Theme`].
/// Only the first call takes effect within the process.
pub(crate) fn init_overrides(section: &ThemeSection) -> Result<()> {
    let parsed = (validate(&section.dark)?, validate(&section.light)?);
    let _ = OVERRIDES.set(parsed);
    Ok(())
}

/// Validate override names, single/pair value shapes, and hexadecimal color syntax.
fn validate(raw: &HashMap<String, ColorValue>) -> Result<Overrides> {
    let mut out = Overrides::new();
    for (name, value) in raw {
        let Some(&(_, is_pair)) = COLOR_FIELDS.iter().find(|(n, _)| n == name) else {
            let list: Vec<&str> = COLOR_FIELDS.iter().map(|(n, _)| *n).collect();
            bail!(
                "{}",
                tr_f(
                    "config.bad_color_name",
                    &[("name", name), ("list", &list.join(", "))],
                )
            );
        };
        let bad = |shown: &str| {
            anyhow::anyhow!(
                "{}",
                tr_f("config.bad_color", &[("name", name), ("value", shown)])
            )
        };
        let parsed = match value {
            ColorValue::Single(hex) if !is_pair => {
                OverrideValue::Single(parse_hex(hex).ok_or_else(|| bad(hex))?)
            }
            ColorValue::Pair([normal, selected]) if is_pair => OverrideValue::Pair(
                parse_hex(normal).ok_or_else(|| bad(normal))?,
                parse_hex(selected).ok_or_else(|| bad(selected))?,
            ),
            ColorValue::Single(hex) => return Err(bad(hex)),
            ColorValue::Pair([normal, _]) => return Err(bad(normal)),
        };
        out.insert(name.clone(), parsed);
    }
    Ok(out)
}

/// UI theme: the single source of all rendering colors.
#[derive(Debug, Clone)]
pub(crate) struct Theme {
    /// Whether this is the light variant, selecting Maple Light / Dark syntax colors
    pub(crate) light: bool,
    /// Secondary text and line numbers
    pub(crate) fg_dim: Color,
    /// Selected chunk line numbers and highlighted text
    pub(crate) fg_bright: Color,
    /// Blue (single-side changes)
    pub(crate) blue: Color,
    /// Green (identical changes / ready state)
    pub(crate) green: Color,
    /// Red (conflicts)
    pub(crate) red: Color,
    /// Amber (messages and hints)
    pub(crate) amber: Color,
    /// Pane borders
    pub(crate) border: Color,
    /// Badge foreground, dark against an accent background
    pub(crate) badge_fg: Color,
    /// Scrollbar thumb
    pub(crate) scrollbar_thumb: Color,
    /// Keycap foreground in the bottom hint bar
    pub(crate) keycap_fg: Color,
    /// Keycap background in the bottom hint bar
    pub(crate) keycap_bg: Color,
    /// Hint descriptions, brighter than fg_dim for readability
    pub(crate) hint_fg: Color,
    /// Rust orange for menu logo characters without embedded colors
    pub(crate) logo: Color,
    /// Pending-resolution placeholder in the result pane
    pub(crate) placeholder_fg: Color,
    /// RPG menu: double-line window borders and embedded titles (off-white)
    pub(crate) rpg_frame: Color,
    /// RPG menu: orange accent for selected rows and cursor symbols
    pub(crate) rpg_accent: Color,
    /// RPG menu: HP gauge (green)
    pub(crate) rpg_hp: Color,
    /// RPG menu: MP gauge (blue)
    pub(crate) rpg_mp: Color,
    /// RPG menu: EXP gauge (yellow-orange)
    pub(crate) rpg_exp: Color,
    /// RPG menu: empty gauge slots
    pub(crate) rpg_gauge_empty: Color,
    /// RPG menu: level number (gold)
    pub(crate) rpg_gold: Color,
    /// Modified band backgrounds (normal, selected)
    band_modified: (Color, Color),
    /// Added band backgrounds (normal, selected)
    band_added: (Color, Color),
    /// Deleted band backgrounds (normal, selected)
    band_deleted: (Color, Color),
    /// Conflict band backgrounds (normal, selected)
    band_conflict: (Color, Color),
    /// Modified word-emphasis backgrounds (normal, selected), brighter than bands in the same hue
    emph_modified: (Color, Color),
    /// Added word-emphasis backgrounds (normal, selected)
    emph_added: (Color, Color),
    /// Deleted word-emphasis backgrounds (normal, selected)
    emph_deleted: (Color, Color),
    /// Conflict word-emphasis backgrounds (normal, selected)
    emph_conflict: (Color, Color),
}

impl Theme {
    /// Select [`Theme::light`] for light terminals, otherwise [`Theme::tokyo_night`],
    /// then apply configured overrides for that variant.
    pub(crate) fn select(light: bool) -> Self {
        let mut theme = if light {
            Self::light()
        } else {
            Self::tokyo_night()
        };
        if let Some((dark_overrides, light_overrides)) = OVERRIDES.get() {
            let overrides = if light {
                light_overrides
            } else {
                dark_overrides
            };
            for (name, value) in overrides {
                theme.set_color(name, *value);
            }
        }
        theme
    }

    /// Set a named color override; return false for names absent from [`COLOR_FIELDS`].
    fn set_color(&mut self, name: &str, value: OverrideValue) -> bool {
        let single = |rgb: (u8, u8, u8)| term_color(rgb.0, rgb.1, rgb.2);
        match (name, value) {
            ("fg_dim", OverrideValue::Single(c)) => self.fg_dim = single(c),
            ("fg_bright", OverrideValue::Single(c)) => self.fg_bright = single(c),
            ("blue", OverrideValue::Single(c)) => self.blue = single(c),
            ("green", OverrideValue::Single(c)) => self.green = single(c),
            ("red", OverrideValue::Single(c)) => self.red = single(c),
            ("amber", OverrideValue::Single(c)) => self.amber = single(c),
            ("border", OverrideValue::Single(c)) => self.border = single(c),
            ("badge_fg", OverrideValue::Single(c)) => self.badge_fg = single(c),
            ("scrollbar_thumb", OverrideValue::Single(c)) => self.scrollbar_thumb = single(c),
            ("keycap_fg", OverrideValue::Single(c)) => self.keycap_fg = single(c),
            ("keycap_bg", OverrideValue::Single(c)) => self.keycap_bg = single(c),
            ("hint_fg", OverrideValue::Single(c)) => self.hint_fg = single(c),
            ("logo", OverrideValue::Single(c)) => self.logo = single(c),
            ("placeholder_fg", OverrideValue::Single(c)) => self.placeholder_fg = single(c),
            ("rpg_frame", OverrideValue::Single(c)) => self.rpg_frame = single(c),
            ("rpg_accent", OverrideValue::Single(c)) => self.rpg_accent = single(c),
            ("rpg_hp", OverrideValue::Single(c)) => self.rpg_hp = single(c),
            ("rpg_mp", OverrideValue::Single(c)) => self.rpg_mp = single(c),
            ("rpg_exp", OverrideValue::Single(c)) => self.rpg_exp = single(c),
            ("rpg_gauge_empty", OverrideValue::Single(c)) => self.rpg_gauge_empty = single(c),
            ("rpg_gold", OverrideValue::Single(c)) => self.rpg_gold = single(c),
            ("band_modified", OverrideValue::Pair(n, s)) => {
                self.band_modified = (single(n), single(s));
            }
            ("band_added", OverrideValue::Pair(n, s)) => self.band_added = (single(n), single(s)),
            ("band_deleted", OverrideValue::Pair(n, s)) => {
                self.band_deleted = (single(n), single(s));
            }
            ("band_conflict", OverrideValue::Pair(n, s)) => {
                self.band_conflict = (single(n), single(s));
            }
            ("emph_modified", OverrideValue::Pair(n, s)) => {
                self.emph_modified = (single(n), single(s));
            }
            ("emph_added", OverrideValue::Pair(n, s)) => self.emph_added = (single(n), single(s)),
            ("emph_deleted", OverrideValue::Pair(n, s)) => {
                self.emph_deleted = (single(n), single(s));
            }
            ("emph_conflict", OverrideValue::Pair(n, s)) => {
                self.emph_conflict = (single(n), single(s));
            }
            _ => return false,
        }
        true
    }

    /// Default Tokyo Night theme.
    pub(crate) fn tokyo_night() -> Self {
        Self {
            light: false,
            fg_dim: term_color(108, 116, 130),
            fg_bright: term_color(205, 214, 244),
            blue: term_color(122, 162, 247),
            green: term_color(158, 206, 106),
            red: term_color(224, 108, 117),
            amber: term_color(229, 192, 123),
            border: term_color(102, 112, 148),
            badge_fg: term_color(16, 18, 24),
            scrollbar_thumb: term_color(160, 168, 192),
            keycap_fg: term_color(205, 214, 244),
            keycap_bg: term_color(45, 50, 66),
            hint_fg: term_color(170, 178, 196),
            logo: term_color(255, 118, 48),
            placeholder_fg: term_color(196, 132, 138),
            rpg_frame: term_color(234, 225, 198),
            rpg_accent: term_color(255, 122, 47),
            rpg_hp: term_color(235, 110, 110),
            rpg_mp: term_color(108, 178, 255),
            rpg_exp: term_color(255, 176, 79),
            rpg_gauge_empty: term_color(76, 90, 122),
            rpg_gold: term_color(255, 195, 92),
            band_modified: (term_color(36, 56, 96), term_color(52, 80, 132)),
            band_added: (term_color(26, 42, 31), term_color(40, 66, 48)),
            band_deleted: (term_color(44, 47, 56), term_color(60, 64, 76)),
            band_conflict: (term_color(58, 30, 34), term_color(94, 45, 53)),
            emph_modified: (term_color(72, 102, 158), term_color(90, 126, 186)),
            emph_added: (term_color(52, 88, 62), term_color(66, 110, 78)),
            emph_deleted: (term_color(76, 81, 94), term_color(94, 100, 116)),
            emph_conflict: (term_color(110, 52, 62), term_color(140, 66, 78)),
        }
    }

    /// Light theme for white terminals, with Maple Light accents and pastel bands.
    pub(crate) fn light() -> Self {
        Self {
            light: true,
            fg_dim: term_color(100, 108, 122),
            fg_bright: term_color(30, 41, 59),
            blue: term_color(5, 133, 168),
            green: term_color(71, 143, 20),
            red: term_color(189, 81, 81),
            amber: term_color(195, 117, 34),
            border: term_color(148, 156, 176),
            badge_fg: term_color(255, 255, 255),
            scrollbar_thumb: term_color(110, 120, 145),
            keycap_fg: term_color(30, 41, 59),
            keycap_bg: term_color(226, 230, 238),
            hint_fg: term_color(71, 85, 105),
            logo: term_color(214, 77, 0),
            placeholder_fg: term_color(158, 70, 76),
            rpg_frame: term_color(90, 74, 47),
            rpg_accent: term_color(217, 95, 16),
            rpg_hp: term_color(196, 72, 72),
            rpg_mp: term_color(36, 110, 196),
            rpg_exp: term_color(200, 120, 20),
            rpg_gauge_empty: term_color(198, 190, 170),
            rpg_gold: term_color(170, 122, 12),
            band_modified: (term_color(219, 233, 249), term_color(196, 219, 244)),
            band_added: (term_color(222, 240, 216), term_color(200, 229, 192)),
            band_deleted: (term_color(229, 231, 236), term_color(212, 215, 223)),
            band_conflict: (term_color(250, 223, 224), term_color(245, 200, 202)),
            emph_modified: (term_color(176, 208, 242), term_color(152, 192, 236)),
            emph_added: (term_color(185, 220, 172), term_color(163, 206, 148)),
            emph_deleted: (term_color(203, 207, 217), term_color(186, 191, 204)),
            emph_conflict: (term_color(240, 178, 181), term_color(233, 154, 158)),
        }
    }

    /// Change band backgrounds: blue=modified, green=added, gray=deleted, red=conflict.
    /// Selected variants increase contrast within the same hue.
    pub(crate) fn band_bg(&self, change: ChangeType, current: bool) -> Option<Color> {
        let (normal, selected) = match change {
            ChangeType::None => return None,
            ChangeType::Modified => self.band_modified,
            ChangeType::Added => self.band_added,
            ChangeType::Deleted => self.band_deleted,
            ChangeType::Conflict => self.band_conflict,
        };
        Some(if current { selected } else { normal })
    }

    /// Word-emphasis background, brighter than the band in the same hue.
    pub(crate) fn emph_bg(&self, change: ChangeType, current: bool) -> Color {
        let (normal, selected) = match change {
            ChangeType::Added => self.emph_added,
            ChangeType::Deleted => self.emph_deleted,
            ChangeType::Conflict => self.emph_conflict,
            _ => self.emph_modified,
        };
        if current { selected } else { normal }
    }

    /// Change accents for gutter symbols, matching the bands' hues.
    pub(crate) fn accent(&self, change: ChangeType) -> Color {
        match change {
            ChangeType::Modified => self.blue,
            ChangeType::Added => self.green,
            ChangeType::Conflict => self.red,
            ChangeType::Deleted | ChangeType::None => self.fg_dim,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::tokyo_night()
    }
}

/// Use RGB on true-color terminals, otherwise quantize to the xterm-256 palette.
///
/// Terminals without 24-bit color, such as macOS Terminal.app, may interpret
/// `38;2;R;G;B` parameters separately, treating R=5/6 as SGR blinking attributes.
/// Quantizing to `38;5;n` avoids this unintended text blinking.
pub(crate) fn term_color(r: u8, g: u8, b: u8) -> Color {
    static TRUECOLOR: OnceLock<bool> = OnceLock::new();
    let truecolor = *TRUECOLOR.get_or_init(|| {
        std::env::var("COLORTERM").is_ok_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("truecolor") || v.contains("24bit")
        })
    });
    if truecolor {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(rgb_to_xterm256(r, g, b))
    }
}

/// Quantize RGB to xterm-256 by comparing the nearest color in the 6x6x6 cube
/// (16-231) with the nearest grayscale entry (232-255), using Euclidean distance.
fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    // Six color-cube levels: 0, 95, 135, 175, 215, 255.
    let level = |v: u8| -> i32 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            i32::from((v - 35) / 40).min(5)
        }
    };
    let level_value = |i: i32| -> i32 { if i == 0 { 0 } else { 55 + 40 * i } };
    let sq = |a: i32, b: i32| (a - b) * (a - b);

    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    let (ri, gi, bi) = (level(r as u8), level(g as u8), level(b as u8));
    let cube_dist = sq(r, level_value(ri)) + sq(g, level_value(gi)) + sq(b, level_value(bi));

    // Twenty-four grayscale levels: 8, 18, ..., 238.
    let avg = (r + g + b) / 3;
    let gray_idx = ((avg - 3) / 10).clamp(0, 23);
    let gray_value = 8 + 10 * gray_idx;
    let gray_dist = sq(r, gray_value) + sq(g, gray_value) + sq(b, gray_value);

    if gray_dist < cube_dist {
        (232 + gray_idx) as u8
    } else {
        (16 + 36 * ri + 6 * gi + bi) as u8
    }
}

/// Detect a light terminal background from the environment, defaulting to dark.
pub(crate) fn detect_light() -> bool {
    std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| parse_colorfgbg(&v))
        .unwrap_or(false)
}

/// Parse `COLORFGBG`, set by konsole / rxvt / mintty as "fg;bg" or "fg;default;bg".
/// The last field is the background color; ANSI colors 7 and 15 are white variants.
fn parse_colorfgbg(value: &str) -> Option<bool> {
    let bg = value.split(';').next_back()?.trim().parse::<u8>().ok()?;
    Some(bg == 7 || bg == 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RGB to xterm-256: exact cube values, grayscale, and black/white endpoints.
    #[test]
    fn rgb_quantizes_to_xterm256() {
        assert_eq!(rgb_to_xterm256(0, 0, 0), 16); // Cube origin.
        assert_eq!(rgb_to_xterm256(255, 255, 255), 231); // Cube white.
        assert_eq!(rgb_to_xterm256(255, 0, 0), 196); // Pure red.
        assert_eq!(rgb_to_xterm256(95, 135, 175), 67); // Exact cube levels (1, 2, 3).
        assert_eq!(rgb_to_xterm256(128, 128, 128), 244); // Mid-gray uses grayscale.
    }

    /// Background detection across COLORFGBG formats.
    #[test]
    fn parse_colorfgbg_variants() {
        assert_eq!(parse_colorfgbg("0;15"), Some(true)); // White background.
        assert_eq!(parse_colorfgbg("15;0"), Some(false)); // Black background.
        assert_eq!(parse_colorfgbg("0;default;7"), Some(true)); // Three-field rxvt format.
        assert_eq!(parse_colorfgbg("12;8"), Some(false)); // Dark gray background.
        assert_eq!(parse_colorfgbg("garbage"), None); // Invalid input.
        assert_eq!(parse_colorfgbg(""), None);
    }

    /// Both themes define all bands and correctly identify the light variant.
    #[test]
    fn both_variants_provide_bands() {
        for theme in [Theme::tokyo_night(), Theme::light()] {
            assert!(theme.band_bg(ChangeType::Conflict, false).is_some());
            assert!(theme.band_bg(ChangeType::None, false).is_none());
        }
        assert!(Theme::select(true).light);
        assert!(!Theme::select(false).light);
    }

    /// Every COLOR_FIELDS name maps to a field; reject incorrect shapes and unknown names.
    #[test]
    fn every_listed_color_is_settable() {
        let mut theme = Theme::tokyo_night();
        for (name, is_pair) in COLOR_FIELDS {
            let (matching, wrong) = if *is_pair {
                (
                    OverrideValue::Pair((1, 2, 3), (4, 5, 6)),
                    OverrideValue::Single((1, 2, 3)),
                )
            } else {
                (
                    OverrideValue::Single((1, 2, 3)),
                    OverrideValue::Pair((1, 2, 3), (4, 5, 6)),
                )
            };
            assert!(
                theme.set_color(name, matching),
                "{name} is not mapped to a field"
            );
            assert!(
                !theme.set_color(name, wrong),
                "{name} accepted the wrong value shape"
            );
        }
        assert!(!theme.set_color("nope", OverrideValue::Single((0, 0, 0))));
    }

    /// Accept valid overrides; report unknown names, invalid hex, and incorrect value shapes.
    #[test]
    fn validate_rejects_bad_overrides() {
        let single = |v: &str| ColorValue::Single(v.to_owned());
        let pair = |a: &str, b: &str| ColorValue::Pair([a.to_owned(), b.to_owned()]);

        let ok = HashMap::from([
            ("rpg_accent".to_owned(), single("#ff7a2f")),
            ("band_conflict".to_owned(), pair("#3a1e22", "#5e2d35")),
        ]);
        assert_eq!(validate(&ok).unwrap().len(), 2);

        let unknown = HashMap::from([("nope".to_owned(), single("#000000"))]);
        let err = validate(&unknown).unwrap_err().to_string();
        assert!(err.contains("nope") && err.contains("rpg_accent"));

        let bad_hex = HashMap::from([("blue".to_owned(), single("#zz0000"))]);
        assert!(validate(&bad_hex).is_err());

        // A pair supplied for a single-color field, and vice versa.
        let wrong_shape = HashMap::from([("blue".to_owned(), pair("#000000", "#111111"))]);
        assert!(validate(&wrong_shape).is_err());
        let wrong_shape = HashMap::from([("band_added".to_owned(), single("#000000"))]);
        assert!(validate(&wrong_shape).is_err());
    }
}
