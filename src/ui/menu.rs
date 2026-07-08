//! 通用单选列表浮层:主操作菜单与分支 / 提交选择器共用。
//!
//! [`MenuSession`] 把多级菜单与弹框收进同一次 ratatui 会话:
//! 页面切换只清屏重绘,不退出 alternate screen,避免闪屏;
//! 会话 Drop 时恢复终端,与随后的 git 透传输出、冲突解决界面顺序衔接。

use std::io::IsTerminal;

use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};

use std::sync::OnceLock;

use super::theme::{Theme, term_color};

/// 主菜单 logo 字符画,编译期嵌入。
///
/// 两种内容都支持:带 ANSI 颜色序列的真彩像素画(保留原色),
/// 或纯文本字符画(渲染时统一着主题 logo 色)。
const LOGO_ANSI: &str = include_str!("logo.ans");

/// 解析并缓存 logo(进程内只解析一次;颜色经 term_color 适配终端能力)。
fn logo_art() -> &'static [Line<'static>] {
    static ART: OnceLock<Vec<Line<'static>>> = OnceLock::new();
    ART.get_or_init(|| parse_ansi_art(LOGO_ANSI))
}

/// 解析 ANSI 字符画为 ratatui 行序列。
///
/// 只支持 logo 用到的 SGR 子集:`0` 重置、`38;2;r;g;b` 前景、
/// `48;2;r;g;b` 背景;无样式的空格视为透明格。
/// 首尾的全空行会被裁剪。
fn parse_ansi_art(src: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<(Line<'static>, bool)> = Vec::new();
    for raw in src.lines() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut visible = false;
        let (mut fg, mut bg): (Option<Style>, Option<Style>) = (None, None);
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // 收集 `[` 与 `m` 之间的参数段
                let mut params = String::new();
                for pc in chars.by_ref() {
                    match pc {
                        '[' => {}
                        'm' => break,
                        other => params.push(other),
                    }
                }
                apply_sgr(&params, &mut fg, &mut bg);
            } else {
                let mut style = Style::new();
                if let Some(f) = fg {
                    style = style.patch(f);
                }
                if let Some(b) = bg {
                    style = style.patch(b);
                }
                // 带颜色或非空白的字符都算可见;纯文本字符画因此不会被裁剪
                if fg.is_some() || bg.is_some() || !c.is_whitespace() {
                    visible = true;
                }
                spans.push(Span::styled(c.to_string(), style));
            }
        }
        lines.push((Line::from(spans), visible));
    }
    // 裁掉首尾无可见像素的行
    while lines.last().is_some_and(|(_, v)| !v) {
        lines.pop();
    }
    let skip = lines.iter().take_while(|(_, v)| !v).count();
    lines.drain(..skip);
    lines.into_iter().map(|(l, _)| l).collect()
}

/// 应用一段 SGR 参数到当前前景 / 背景状态。
fn apply_sgr(params: &str, fg: &mut Option<Style>, bg: &mut Option<Style>) {
    let nums: Vec<u16> = params
        .split(';')
        .filter_map(|n| n.parse::<u16>().ok())
        .collect();
    let mut i = 0;
    while i < nums.len() {
        match nums[i] {
            0 => {
                *fg = None;
                *bg = None;
                i += 1;
            }
            code @ (38 | 48) if nums.len() >= i + 5 && nums[i + 1] == 2 => {
                let (r, g, b) = (nums[i + 2] as u8, nums[i + 3] as u8, nums[i + 4] as u8);
                let color = term_color(r, g, b);
                if code == 38 {
                    *fg = Some(Style::new().fg(color));
                } else {
                    *bg = Some(Style::new().bg(color));
                }
                i += 5;
            }
            _ => i += 1,
        }
    }
}

/// 菜单条目:标签(命令 / 分支 / 短 hash)+ 可选描述,渲染时分色两列。
#[derive(Debug, Clone)]
pub(crate) struct MenuItem {
    /// 左列标签(选择结果以此为准)
    pub(crate) label: String,
    /// 右列描述;为空时只渲染标签
    pub(crate) desc: String,
}

impl MenuItem {
    /// 构造条目。
    pub(crate) fn new(label: impl Into<String>, desc: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            desc: desc.into(),
        }
    }
}

/// 估算终端显示宽度(CJK 记 2 列,其余记 1 列)。
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

/// 光标回绕移动:`delta` 为 +1 / -1。
fn wrap_move(cursor: usize, len: usize, delta: isize) -> usize {
    (cursor as isize + delta).rem_euclid(len as isize) as usize
}

/// 一次连续的菜单 TUI 会话:多级选择页与弹框共享同一终端现场。
///
/// 页面切换只清屏重绘,不反复进出 alternate screen,避免闪屏;
/// Drop 时恢复终端,因此执行需要透传输出的 git 命令前应先结束会话。
pub(crate) struct MenuSession {
    /// 会话持有的终端(Drop 时统一恢复)
    terminal: ratatui::DefaultTerminal,
    /// 界面主题
    theme: Theme,
}

impl MenuSession {
    /// 打开菜单会话(进入 TUI);非交互终端直接报错。
    pub(crate) fn open(light: bool) -> Result<Self> {
        if !std::io::stdout().is_terminal() {
            anyhow::bail!("打开选择菜单需要交互式终端(当前 stdout 不是 TTY)");
        }
        Ok(Self {
            terminal: ratatui::init(),
            theme: Theme::select(light),
        })
    }

    /// 运行单选列表页;返回选中项下标,None 表示取消(q / Esc)或列表为空。
    /// `logo` 为 true 时在列表上方绘制 Ferris(主菜单用);
    /// `initial` 为光标初始位置(从下级页面返回时停在上次的选项上)。
    pub(crate) fn pick(
        &mut self,
        title: &str,
        items: &[MenuItem],
        logo: bool,
        initial: usize,
    ) -> Result<Option<usize>> {
        if items.is_empty() {
            return Ok(None);
        }
        // 清屏强制全量重绘,抹掉上一页的残留(各页面板尺寸不同)
        self.terminal.clear()?;
        pick_loop(&mut self.terminal, title, items, logo, initial, &self.theme)
    }

    /// 展示消息弹框(如失败原因),任意键关闭。
    pub(crate) fn notice(&mut self, title: &str, body: &str) -> Result<()> {
        self.terminal.clear()?;
        notice_loop(&mut self.terminal, title, body, &self.theme)
    }

    /// 绘制一帧居中提示后立即返回(不等待按键),
    /// 供随后的阻塞操作(如需要网络的 git 命令)期间维持画面反馈。
    pub(crate) fn flash(&mut self, body: &str) -> Result<()> {
        self.terminal.clear()?;
        self.terminal.draw(|frame| {
            let area = frame.area();
            if area.height == 0 {
                return;
            }
            let line_area = Rect {
                x: area.x,
                y: area.y + area.height / 2,
                width: area.width,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(
                    Line::styled(body.to_owned(), Style::new().fg(self.theme.amber)).centered(),
                ),
                line_area,
            );
        })?;
        Ok(())
    }
}

impl Drop for MenuSession {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// 选择器事件循环:绘制 → 读键 → 移动 / 确认 / 取消。
fn pick_loop(
    terminal: &mut ratatui::DefaultTerminal,
    title: &str,
    items: &[MenuItem],
    logo: bool,
    initial: usize,
    theme: &Theme,
) -> Result<Option<usize>> {
    let mut cursor = initial.min(items.len() - 1);
    // ListState 跨帧保留滚动偏移,选中项始终保持在视口内
    let mut list = ListState::default();
    loop {
        list.select(Some(cursor));
        terminal.draw(|frame| draw_pick(frame, title, items, logo, &mut list, theme))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => cursor = wrap_move(cursor, items.len(), 1),
            KeyCode::Char('k') | KeyCode::Up => cursor = wrap_move(cursor, items.len(), -1),
            KeyCode::Enter => return Ok(Some(cursor)),
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            _ => {}
        }
    }
}

/// 绘制居中的选择浮层:圆角边框 + 内边距 + 可选 Ferris logo
/// + 两列分色列表 + 底部按键提示。
fn draw_pick(
    frame: &mut Frame,
    title: &str,
    items: &[MenuItem],
    logo: bool,
    list_state: &mut ListState,
    theme: &Theme,
) {
    let area = frame.area();
    // 标签列宽对齐到最长标签;面板宽度按内容自适应,不超出屏幕
    let label_w = items
        .iter()
        .map(|i| display_width(&i.label))
        .max()
        .unwrap_or(0);
    let content_w = items
        .iter()
        .map(|i| {
            let desc = display_width(&i.desc);
            label_w + if desc == 0 { 0 } else { 2 + desc }
        })
        .max()
        .unwrap_or(0);
    // logo 画在菜单框上方,与菜单成组垂直居中;屏幕放不下时自动隐藏。
    // 逐行内容左缘对齐(整块统一偏移居中),避免行尾透明格差异造成错位
    let art = logo_art();
    let logo_w = art.iter().map(Line::width).max().unwrap_or(0) as u16;
    let logo_h = art.len() as u16 + 1;
    // 边框 2 + 左右内边距 4 + 选中符号 2;主菜单加宽到与 logo 齐宽
    let fit_w = (content_w + 8) as u16;
    let width = if logo && !art.is_empty() {
        fit_w.max(logo_w + 2)
    } else {
        fit_w.max(36)
    }
    .min(area.width.saturating_sub(4));
    // 边框 2 + 上下内边距 2
    let panel_h = (items.len() as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(5);
    let show_logo =
        logo && !art.is_empty() && area.height > panel_h + logo_h + 2 && area.width >= logo_w;
    let total_h = panel_h + if show_logo { logo_h } else { 0 };
    let top = area.y + area.height.saturating_sub(total_h) / 2;
    if show_logo {
        let logo_area = Rect {
            x: area.x + area.width.saturating_sub(logo_w) / 2,
            y: top,
            width: logo_w,
            height: art.len() as u16,
        };
        // 无自带颜色的字符统一着主题 logo 色(纯文本字符画场景)
        let tinted: Vec<Line<'static>> = art
            .iter()
            .map(|line| {
                let spans: Vec<Span<'static>> = line
                    .spans
                    .iter()
                    .map(|s| {
                        if s.style.fg.is_none()
                            && s.style.bg.is_none()
                            && !s.content.trim().is_empty()
                        {
                            Span::styled(s.content.clone(), Style::new().fg(theme.logo))
                        } else {
                            s.clone()
                        }
                    })
                    .collect();
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(tinted), logo_area);
    }
    let panel = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: top + if show_logo { logo_h } else { 0 },
        width: width.min(area.width),
        height: panel_h,
    };

    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .padding(Padding::new(2, 2, 1, 1))
        .title_bottom(
            Line::from(Span::styled(
                " j/k 移动 · Enter 确认 · q 取消 ",
                Style::new().fg(theme.fg_dim),
            ))
            .centered(),
        );
    // 标题可选:主菜单不设标题(logo 已表明身份),二级列表用标题说明语境
    if !title.trim().is_empty() {
        block = block.title(Span::styled(
            format!(" {} ", title.trim()),
            Style::new().fg(theme.blue).add_modifier(Modifier::BOLD),
        ));
    }

    // 列表内容可用宽度 = 面板宽 - 边框 2 - 内边距 4 - 选中符号 2
    let inner_w = width.saturating_sub(8) as usize;
    let rows: Vec<ListItem<'static>> = items
        .iter()
        .map(|item| {
            let mut spans = vec![Span::styled(
                format!(
                    "{}{}",
                    item.label,
                    " ".repeat(label_w.saturating_sub(display_width(&item.label)))
                ),
                Style::new().fg(theme.blue),
            )];
            if !item.desc.is_empty() {
                // 主菜单标签与描述分居两端(面板宽);普通列表描述紧随标签
                let gap = if logo {
                    inner_w
                        .saturating_sub(label_w + display_width(&item.desc))
                        .max(2)
                } else {
                    2
                };
                spans.push(Span::raw(" ".repeat(gap)));
                spans.push(Span::styled(
                    item.desc.clone(),
                    Style::new().fg(theme.hint_fg),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    // 整行高亮:只改背景并加粗,保留两列各自的前景色
    let list = List::new(rows)
        .block(block)
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::new()
                .bg(theme.keycap_bg)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(Clear, panel);
    frame.render_stateful_widget(list, panel, list_state);
}

/// 弹框事件循环:绘制一次,等待任意按键关闭。
fn notice_loop(
    terminal: &mut ratatui::DefaultTerminal,
    title: &str,
    body: &str,
    theme: &Theme,
) -> Result<()> {
    loop {
        terminal.draw(|frame| draw_notice(frame, title, body, theme))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            return Ok(());
        }
    }
}

/// 绘制居中的消息弹框:琥珀色标题 + 自动换行正文。
fn draw_notice(frame: &mut Frame, title: &str, body: &str, theme: &Theme) {
    let area = frame.area();
    let width = 64u16.min(area.width.saturating_sub(4)).max(20);
    let inner_w = width.saturating_sub(6) as usize;
    // 估算换行后的行数以确定弹框高度
    let lines: usize = body
        .lines()
        .map(|l| display_width(l).div_ceil(inner_w.max(1)).max(1))
        .sum();
    let height = ((lines as u16) + 4)
        .min(area.height.saturating_sub(2))
        .max(5);
    let panel = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .padding(Padding::new(2, 2, 1, 1))
        .title(Span::styled(
            format!(" {} ", title.trim()),
            Style::new().fg(theme.amber).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                " 按任意键关闭 ",
                Style::new().fg(theme.fg_dim),
            ))
            .centered(),
        );
    frame.render_widget(Clear, panel);
    frame.render_widget(
        Paragraph::new(body.to_owned())
            .wrap(Wrap { trim: false })
            .block(block),
        panel,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 光标回绕移动
    #[test]
    fn wrap_move_cycles() {
        assert_eq!(wrap_move(0, 5, -1), 4); // 顶部上移回绕到底
        assert_eq!(wrap_move(4, 5, 1), 0); // 底部下移回绕到顶
        assert_eq!(wrap_move(2, 5, 1), 3);
    }

    /// CJK 显示宽度估算(面板宽度自适应用)
    #[test]
    fn display_width_counts_cjk_as_two() {
        assert_eq!(display_width("pull"), 4);
        assert_eq!(display_width("拉取"), 4);
        assert_eq!(display_width("a拉b"), 4);
    }

    /// ANSI 半块像素画解析:颜色状态机、透明格与尾部空行裁剪
    #[test]
    fn parses_ansi_half_block_art() {
        let src = "\x1b[0m \x1b[38;2;10;20;30m\x1b[48;2;40;50;60m▀\x1b[0m \n\x1b[0m \x1b[0m ";
        let lines = parse_ansi_art(src);
        assert_eq!(lines.len(), 1, "尾部全透明行应被裁剪");
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 3); // 透明格 + 像素 + 透明格
        assert_eq!(spans[1].content, "▀");
        assert!(spans[1].style.fg.is_some());
        assert!(spans[1].style.bg.is_some());
        assert!(spans[0].style.fg.is_none(), "透明格应无样式");
    }

    /// 纯文本字符画(无 ANSI 颜色)不应被裁剪
    #[test]
    fn plain_text_art_is_not_trimmed() {
        let lines = parse_ansi_art("ABC\n\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans.len(), 3);
    }

    /// 内嵌 logo 资产能解析出非空字符画
    #[test]
    fn embedded_logo_parses() {
        let art = logo_art();
        assert!(!art.is_empty());
        assert!(art.iter().map(Line::width).max().unwrap_or(0) > 20);
    }
}
