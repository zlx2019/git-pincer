//! 界面主题:全部颜色的集中定义,默认 Tokyo Night(暗色终端调校)。
//!
//! 选中提亮遵循「同色相加深增饱和」而非均匀加灰,避免颜色发浑。

use ratatui::style::Color;

use super::rows::ChangeType;

/// 界面主题:所有渲染颜色的单一来源。
#[derive(Debug, Clone)]
pub(crate) struct Theme {
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
    /// 结果栏「待解决」占位文字
    pub(crate) placeholder_fg: Color,
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
    /// Tokyo Night 默认主题。
    pub(crate) fn tokyo_night() -> Self {
        Self {
            fg_dim: Color::Rgb(108, 116, 130),
            fg_bright: Color::Rgb(205, 214, 244),
            blue: Color::Rgb(122, 162, 247),
            green: Color::Rgb(158, 206, 106),
            red: Color::Rgb(224, 108, 117),
            amber: Color::Rgb(229, 192, 123),
            border: Color::Rgb(102, 112, 148),
            badge_fg: Color::Rgb(16, 18, 24),
            scrollbar_thumb: Color::Rgb(160, 168, 192),
            keycap_fg: Color::Rgb(205, 214, 244),
            keycap_bg: Color::Rgb(45, 50, 66),
            placeholder_fg: Color::Rgb(196, 132, 138),
            band_modified: (Color::Rgb(28, 39, 58), Color::Rgb(45, 64, 96)),
            band_added: (Color::Rgb(26, 42, 31), Color::Rgb(40, 66, 48)),
            band_deleted: (Color::Rgb(44, 47, 56), Color::Rgb(60, 64, 76)),
            band_conflict: (Color::Rgb(58, 30, 34), Color::Rgb(94, 45, 53)),
            emph_modified: (Color::Rgb(58, 84, 130), Color::Rgb(74, 106, 160)),
            emph_added: (Color::Rgb(52, 88, 62), Color::Rgb(66, 110, 78)),
            emph_deleted: (Color::Rgb(76, 81, 94), Color::Rgb(94, 100, 116)),
            emph_conflict: (Color::Rgb(110, 52, 62), Color::Rgb(140, 66, 78)),
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
