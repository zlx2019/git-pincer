//! 界面主题:全部颜色的集中定义。
//!
//! 深色默认 Tokyo Night(暗色终端调校),浅色为 Maple Light 系配套色板;
//! 选中提亮遵循「同色相加深增饱和」而非均匀加灰,避免颜色发浑。
//! 用户可通过配置文件 `[theme.dark]` / `[theme.light]` 按颜色名覆盖,
//! 覆盖在 [`Theme::select`] 构建时应用。

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{Result, bail};
use ratatui::style::Color;

use crate::config::{ColorValue, ThemeSection, parse_hex};
use crate::i18n::tr_f;

use super::rows::ChangeType;

/// 可覆盖的颜色名全集:(名字, 是否为「(普通, 选中)」双色对)。
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

/// 解析后的单条覆盖值。
#[derive(Debug, Clone, Copy)]
enum OverrideValue {
    /// 单色
    Single((u8, u8, u8)),
    /// (普通, 选中) 双色
    Pair((u8, u8, u8), (u8, u8, u8)),
}

/// 深 / 浅两套已解析的覆盖表。
type Overrides = HashMap<String, OverrideValue>;

/// 进程内的主题覆盖(init 冻结;未 init 视同无覆盖)。
static OVERRIDES: OnceLock<(Overrides, Overrides)> = OnceLock::new();

/// 校验并应用配置的 `[theme]` 覆盖(进程内仅首次调用生效,
/// 应在构建任何 [`Theme`] 之前调用)。
pub(crate) fn init_overrides(section: &ThemeSection) -> Result<()> {
    let parsed = (validate(&section.dark)?, validate(&section.light)?);
    let _ = OVERRIDES.set(parsed);
    Ok(())
}

/// 校验一套覆盖:颜色名必须存在,值形态(单色 / 双色)与字段匹配且为合法 hex。
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

/// 界面主题:所有渲染颜色的单一来源。
#[derive(Debug, Clone)]
pub(crate) struct Theme {
    /// 是否为浅色变体(语法高亮据此选择 Maple Light / Dark)
    pub(crate) light: bool,
    /// 次要文字 / 行号
    pub(crate) fg_dim: Color,
    /// 选中块的行号 / 高亮文字
    pub(crate) fg_bright: Color,
    /// 蓝(单侧改动)
    pub(crate) blue: Color,
    /// 绿(一致改动 / 就绪)
    pub(crate) green: Color,
    /// 红(冲突)
    pub(crate) red: Color,
    /// 琥珀(消息与提示)
    pub(crate) amber: Color,
    /// 面板边框
    pub(crate) border: Color,
    /// 徽章前景(深色,配合强调色背景)
    pub(crate) badge_fg: Color,
    /// 滚动条滑块
    pub(crate) scrollbar_thumb: Color,
    /// 键帽前景(底部提示条)
    pub(crate) keycap_fg: Color,
    /// 键帽背景(底部提示条)
    pub(crate) keycap_bg: Color,
    /// 提示条描述文字(比 fg_dim 亮一档,保证可读)
    pub(crate) hint_fg: Color,
    /// 菜单 logo 中无自带颜色字符的着色(Rust 橙)
    pub(crate) logo: Color,
    /// 结果栏「待解决」占位文字
    pub(crate) placeholder_fg: Color,
    /// RPG 菜单:双线窗框与嵌入标题(米白)
    pub(crate) rpg_frame: Color,
    /// RPG 菜单:强调橙(选中行 / 光标符号)
    pub(crate) rpg_accent: Color,
    /// RPG 菜单:HP 条(绿)
    pub(crate) rpg_hp: Color,
    /// RPG 菜单:MP 条(蓝)
    pub(crate) rpg_mp: Color,
    /// RPG 菜单:EXP 条(橙黄)
    pub(crate) rpg_exp: Color,
    /// RPG 菜单:状态条空槽
    pub(crate) rpg_gauge_empty: Color,
    /// RPG 菜单:等级数字(金)
    pub(crate) rpg_gold: Color,
    /// 色带背景:修改(普通, 选中)
    band_modified: (Color, Color),
    /// 色带背景:新增(普通, 选中)
    band_added: (Color, Color),
    /// 色带背景:删除(普通, 选中)
    band_deleted: (Color, Color),
    /// 色带背景:冲突(普通, 选中)
    band_conflict: (Color, Color),
    /// 词级强调背景:修改(普通, 选中),同色相比色带亮两档
    emph_modified: (Color, Color),
    /// 词级强调背景:新增(普通, 选中)
    emph_added: (Color, Color),
    /// 词级强调背景:删除(普通, 选中)
    emph_deleted: (Color, Color),
    /// 词级强调背景:冲突(普通, 选中)
    emph_conflict: (Color, Color),
}

impl Theme {
    /// 按变体选择主题:浅色终端用 [`Theme::light`],否则 [`Theme::tokyo_night`];
    /// 随后应用配置文件的同变体颜色覆盖。
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

    /// 按名字写入一个颜色覆盖;未知名字返回 false(名字集见 [`COLOR_FIELDS`])。
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

    /// Tokyo Night 默认主题。
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

    /// 浅色主题(适配白底终端;强调色取自 Maple Light 系,色带为浅色粉彩)。
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

    /// 各改动类型的色带背景(IDEA 语义:蓝=修改、绿=新增、灰=删除、红=冲突);
    /// 选中版同色相加深提亮。
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

    /// 词级强调的背景色(比色带亮两档,同色相)。
    pub(crate) fn emph_bg(&self, change: ChangeType, current: bool) -> Color {
        let (normal, selected) = match change {
            ChangeType::Added => self.emph_added,
            ChangeType::Deleted => self.emph_deleted,
            ChangeType::Conflict => self.emph_conflict,
            _ => self.emph_modified,
        };
        if current { selected } else { normal }
    }

    /// 各改动类型的强调色(gutter 符号使用,与色带同色相)。
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

/// 依终端能力生成颜色:支持真彩时直出 RGB,否则量化到 xterm-256 调色板。
///
/// 不支持 24-bit 色的终端(如 macOS Terminal.app)会把 `38;2;R;G;B`
/// 的参数拆开逐个解释,R 恰为 5/6 时会被当成 SGR 闪烁属性,
/// 表现为部分文字持续闪烁;量化为 `38;5;n` 的 256 色则所有终端都能正确解析。
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

/// 将 RGB 量化到 xterm-256 调色板:16-231 的 6×6×6 色立方与
/// 232-255 的灰阶各取最近色,再按欧氏距离二选一。
fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    // 色立方 6 级色阶:0, 95, 135, 175, 215, 255
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

    // 灰阶 24 级:8, 18, …, 238
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

/// 从环境推断终端是否为浅色背景;检测不到时按深色处理。
pub(crate) fn detect_light() -> bool {
    std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| parse_colorfgbg(&v))
        .unwrap_or(false)
}

/// 解析 `COLORFGBG`(konsole / rxvt / mintty 等设置,格式 "fg;bg" 或
/// "fg;default;bg"):最后一段为背景色号,ANSI 惯例 7 / 15 为白系。
fn parse_colorfgbg(value: &str) -> Option<bool> {
    let bg = value.split(';').next_back()?.trim().parse::<u8>().ok()?;
    Some(bg == 7 || bg == 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RGB → xterm-256 量化:色立方精确值、灰阶、黑白端点
    #[test]
    fn rgb_quantizes_to_xterm256() {
        assert_eq!(rgb_to_xterm256(0, 0, 0), 16); // 色立方原点
        assert_eq!(rgb_to_xterm256(255, 255, 255), 231); // 色立方白
        assert_eq!(rgb_to_xterm256(255, 0, 0), 196); // 纯红
        assert_eq!(rgb_to_xterm256(95, 135, 175), 67); // 精确落在色阶 (1,2,3)
        assert_eq!(rgb_to_xterm256(128, 128, 128), 244); // 中灰走灰阶
    }

    /// COLORFGBG 各种格式的背景色判定
    #[test]
    fn parse_colorfgbg_variants() {
        assert_eq!(parse_colorfgbg("0;15"), Some(true)); // 白底
        assert_eq!(parse_colorfgbg("15;0"), Some(false)); // 黑底
        assert_eq!(parse_colorfgbg("0;default;7"), Some(true)); // rxvt 三段式
        assert_eq!(parse_colorfgbg("12;8"), Some(false)); // 深灰底
        assert_eq!(parse_colorfgbg("garbage"), None); // 无法解析
        assert_eq!(parse_colorfgbg(""), None);
    }

    /// 两套主题的色带都齐全,且浅色变体的标志位正确
    #[test]
    fn both_variants_provide_bands() {
        for theme in [Theme::tokyo_night(), Theme::light()] {
            assert!(theme.band_bg(ChangeType::Conflict, false).is_some());
            assert!(theme.band_bg(ChangeType::None, false).is_none());
        }
        assert!(Theme::select(true).light);
        assert!(!Theme::select(false).light);
    }

    /// COLOR_FIELDS 中的每个名字都能实际写入;形态不匹配与未知名被拒
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
            assert!(theme.set_color(name, matching), "{name} 未接线到字段");
            assert!(!theme.set_color(name, wrong), "{name} 接受了错误形态");
        }
        assert!(!theme.set_color("nope", OverrideValue::Single((0, 0, 0))));
    }

    /// 覆盖校验:合法输入通过;未知名 / 非法 hex / 形态错误给出可读错误
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

        // 单色字段给了双色对 / 双色字段给了单色
        let wrong_shape = HashMap::from([("blue".to_owned(), pair("#000000", "#111111"))]);
        assert!(validate(&wrong_shape).is_err());
        let wrong_shape = HashMap::from([("band_added".to_owned(), single("#000000"))]);
        assert!(validate(&wrong_shape).is_err());
    }
}
