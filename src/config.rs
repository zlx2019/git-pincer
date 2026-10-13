//! User configuration file: locating, loading and parsing.
//!
//! The file lives at `$XDG_CONFIG_HOME/git-pincer/config.toml` (falling back
//! to `~/.config/git-pincer/config.toml`, macOS included, following the
//! lazygit/delta convention) or `%APPDATA%\git-pincer\config.toml` on
//! Windows; `GIT_PINCER_CONFIG` overrides the path entirely. A missing file
//! yields the defaults; a malformed one fails fast with a readable error.
//! Scope is deliberately behavior-only: theme, key bindings and CLI-option
//! defaults — UI texts are not configurable.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cli::{LangArg, ThemeArg};
use crate::i18n::tr_f;

/// Root structure of the user configuration file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `[ui]`: defaults for global CLI options
    #[serde(default)]
    pub ui: UiSection,
    /// `[keys]`: key overrides (action name -> key description)
    #[serde(default)]
    pub keys: HashMap<String, String>,
    /// `[theme.dark]` / `[theme.light]`: theme color overrides
    #[serde(default)]
    pub theme: ThemeSection,
}

/// `[ui]` section: defaults for matching CLI options, which take precedence.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiSection {
    /// UI theme (auto | dark | light)
    pub theme: Option<ThemeArg>,
    /// UI language (auto | zh | en)
    pub lang: Option<LangArg>,
    /// Whether to echo executed Git commands
    pub verbose: Option<bool>,
    /// Editor command for chunk editing (e key); may include arguments such as "code --wait".
    /// Precedence: this setting > $VISUAL > $EDITOR > vim / vi (notepad on Windows)
    pub editor: Option<String>,
}

/// `[theme]` section: color overrides for dark and light themes (color name -> value).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSection {
    /// Dark theme overrides
    #[serde(default)]
    pub dark: HashMap<String, ColorValue>,
    /// Light theme overrides
    #[serde(default)]
    pub light: HashMap<String, ColorValue>,
}

/// A color: `"#RRGGBB"`, or a `["#normal", "#selected"]` pair for bands and emphasis.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ColorValue {
    /// A single foreground or background color
    Single(String),
    /// A (normal, selected) color pair
    Pair([String; 2]),
}

/// Parse a `#RRGGBB` hexadecimal color into an RGB tuple.
pub fn parse_hex(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// Locate the configuration file; return None if the user directory is unavailable.
pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GIT_PINCER_CONFIG") {
        return Some(PathBuf::from(path));
    }
    #[cfg(windows)]
    {
        Some(
            PathBuf::from(std::env::var_os("APPDATA")?)
                .join("git-pincer")
                .join("config.toml"),
        )
    }
    #[cfg(not(windows))]
    {
        resolve_unix_path(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            std::env::home_dir(),
        )
    }
}

/// Resolve Unix paths: prefer `$XDG_CONFIG_HOME`, otherwise use `~/.config`.
#[cfg(not(windows))]
fn resolve_unix_path(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = match xdg {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => home?.join(".config"),
    };
    Some(base.join("git-pincer").join("config.toml"))
}

/// Load configuration: use defaults for a missing file and report readable parse errors.
pub fn load() -> Result<Config> {
    let Some(path) = config_path() else {
        return Ok(Config::default());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => {
            return Err(e).with_context(|| {
                tr_f(
                    "config.unreadable",
                    &[("path", &path.display().to_string())],
                )
            });
        }
    };
    parse(&text).with_context(|| tr_f("config.invalid", &[("path", &path.display().to_string())]))
}

/// Parse TOML configuration (the pure, testable part of load).
fn parse(text: &str) -> Result<Config> {
    Ok(toml::from_str(text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse all configuration sections and both color value forms.
    #[test]
    fn parses_full_config() {
        let config = parse(
            r##"
[ui]
theme = "dark"
lang = "zh"
verbose = true

[keys]
take-local = "o"
write = "ctrl+s"

[theme.dark]
rpg_accent = "#ff7a2f"
band_conflict = ["#3a1e22", "#5e2d35"]
"##,
        )
        .unwrap();
        assert_eq!(config.ui.theme, Some(ThemeArg::Dark));
        assert_eq!(config.ui.lang, Some(LangArg::Zh));
        assert_eq!(config.ui.verbose, Some(true));
        assert_eq!(config.keys["take-local"], "o");
        assert!(matches!(
            config.theme.dark["rpg_accent"],
            ColorValue::Single(_)
        ));
        assert!(matches!(
            config.theme.dark["band_conflict"],
            ColorValue::Pair(_)
        ));
    }

    /// Empty input and missing sections use defaults.
    #[test]
    fn empty_config_is_default() {
        let config = parse("").unwrap();
        assert!(config.ui.theme.is_none());
        assert!(config.keys.is_empty());
        assert!(config.theme.dark.is_empty());
    }

    /// Reject unknown fields and include their names in errors to identify typos.
    #[test]
    fn unknown_fields_are_rejected() {
        let err = parse("[ui]\nthem = \"dark\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("them"));
    }

    /// Invalid enum values report the available choices.
    #[test]
    fn invalid_enum_lists_variants() {
        let err = parse("[ui]\ntheme = \"drak\"\n").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("drak") && msg.contains("dark"));
    }

    /// Hexadecimal color parsing and invalid input.
    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#ff7a2f"), Some((0xff, 0x7a, 0x2f)));
        assert_eq!(parse_hex("#FFF"), None);
        assert_eq!(parse_hex("ff7a2f"), None);
        assert_eq!(parse_hex("#gg0000"), None);
    }

    /// Unix paths: prefer XDG, fall back to ~/.config for empty values, and require home.
    #[cfg(not(windows))]
    #[test]
    fn unix_path_resolution() {
        let xdg = resolve_unix_path(Some(PathBuf::from("/xdg")), Some(PathBuf::from("/home/z")));
        assert_eq!(xdg, Some(PathBuf::from("/xdg/git-pincer/config.toml")));
        let fallback = resolve_unix_path(None, Some(PathBuf::from("/home/z")));
        assert_eq!(
            fallback,
            Some(PathBuf::from("/home/z/.config/git-pincer/config.toml"))
        );
        let empty_xdg = resolve_unix_path(Some(PathBuf::new()), Some(PathBuf::from("/home/z")));
        assert_eq!(
            empty_xdg,
            Some(PathBuf::from("/home/z/.config/git-pincer/config.toml"))
        );
        assert_eq!(resolve_unix_path(None, None), None);
    }
}
