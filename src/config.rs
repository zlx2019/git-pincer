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

/// 用户配置文件的根结构。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `[ui]`:全局命令参数的默认值
    #[serde(default)]
    pub ui: UiSection,
    /// `[keys]`:按键覆盖(动作名 → 键位描述)
    #[serde(default)]
    pub keys: HashMap<String, String>,
    /// `[theme.dark]` / `[theme.light]`:主题色覆盖
    #[serde(default)]
    pub theme: ThemeSection,
}

/// `[ui]` 段:与同名命令行参数等价,优先级低于命令行。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiSection {
    /// 界面主题(auto | dark | light)
    pub theme: Option<ThemeArg>,
    /// 界面语言(auto | zh | en)
    pub lang: Option<LangArg>,
    /// 是否回显执行的 git 命令
    pub verbose: Option<bool>,
    /// 块编辑(e 键)使用的编辑器命令,可含参数(如 "code --wait");
    /// 完整优先级:此处 > $VISUAL > $EDITOR > vim / vi(Windows 为 notepad)
    pub editor: Option<String>,
}

/// `[theme]` 段:深 / 浅两套主题的颜色覆盖(颜色名 → 值)。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSection {
    /// 深色主题覆盖
    #[serde(default)]
    pub dark: HashMap<String, ColorValue>,
    /// 浅色主题覆盖
    #[serde(default)]
    pub light: HashMap<String, ColorValue>,
}

/// 颜色值:单色 `"#RRGGBB"`,或色带 / 强调类的 `["#普通", "#选中"]` 双色。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ColorValue {
    /// 单个前景 / 背景色
    Single(String),
    /// (普通, 选中) 双色对
    Pair([String; 2]),
}

/// 解析 `#RRGGBB` 十六进制颜色为 RGB 三元组。
pub fn parse_hex(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// 定位配置文件路径;无法确定用户目录时返回 None(视同无配置)。
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

/// Unix 系路径解析:`$XDG_CONFIG_HOME` 优先,否则 `~/.config`。
#[cfg(not(windows))]
fn resolve_unix_path(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = match xdg {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => home?.join(".config"),
    };
    Some(base.join("git-pincer").join("config.toml"))
}

/// 读取并解析配置:文件不存在返回默认值,内容非法则报可读错误。
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

/// 从 TOML 文本解析配置(load 的纯逻辑部分,便于测试)。
fn parse(text: &str) -> Result<Config> {
    Ok(toml::from_str(text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 完整配置解析:各段落与两种颜色值形态
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

    /// 空文本与缺省段落都取默认值
    #[test]
    fn empty_config_is_default() {
        let config = parse("").unwrap();
        assert!(config.ui.theme.is_none());
        assert!(config.keys.is_empty());
        assert!(config.theme.dark.is_empty());
    }

    /// 未知字段被拒绝,错误信息包含字段名(拼写错误可定位)
    #[test]
    fn unknown_fields_are_rejected() {
        let err = parse("[ui]\nthem = \"dark\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("them"));
    }

    /// 非法枚举值给出候选列表
    #[test]
    fn invalid_enum_lists_variants() {
        let err = parse("[ui]\ntheme = \"drak\"\n").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("drak") && msg.contains("dark"));
    }

    /// 十六进制颜色解析与非法输入
    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#ff7a2f"), Some((0xff, 0x7a, 0x2f)));
        assert_eq!(parse_hex("#FFF"), None);
        assert_eq!(parse_hex("ff7a2f"), None);
        assert_eq!(parse_hex("#gg0000"), None);
    }

    /// Unix 路径解析:XDG 优先,空值回退 ~/.config,无 home 返回 None
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
