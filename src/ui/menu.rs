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

use crate::git::RepoVitals;

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
    /// 主菜单底部说明窗的长描述;为空时回退到 desc
    pub(crate) hint: String,
}

impl MenuItem {
    /// 构造条目。
    pub(crate) fn new(label: impl Into<String>, desc: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            desc: desc.into(),
            hint: String::new(),
        }
    }

    /// 附加说明窗长描述。
    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
}

/// 估算终端显示宽度(CJK 记 2 列;箭头 / 制表 / 块元素 / 几何符号记 1 列)。
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            if c.is_ascii() || ('\u{2190}'..='\u{25FF}').contains(&c) {
                1
            } else {
                2
            }
        })
        .sum()
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
    /// `vitals` 为 Some 时渲染完整 RPG 主菜单页(字标 + 状态窗 + 说明窗),
    /// 否则渲染普通双线框列表;`initial` 为光标初始位置。
    pub(crate) fn pick(
        &mut self,
        title: &str,
        items: &[MenuItem],
        vitals: Option<&RepoVitals>,
        initial: usize,
    ) -> Result<Option<usize>> {
        if items.is_empty() {
            return Ok(None);
        }
        // 清屏强制全量重绘,抹掉上一页的残留(各页面板尺寸不同)
        self.terminal.clear()?;
        pick_loop(
            &mut self.terminal,
            title,
            items,
            vitals,
            initial,
            &self.theme,
        )
    }

    /// 展示消息弹框(如失败原因),任意键关闭。
    pub(crate) fn notice(&mut self, title: &str, body: &str) -> Result<()> {
        self.terminal.clear()?;
        notice_loop(&mut self.terminal, title, body, &self.theme)
    }

    /// 绘制一帧「命令执行中」等待页后立即返回(不等待按键),
    /// 供随后的阻塞操作(如需要网络的 git 命令)期间维持画面反馈。
    pub(crate) fn flash(&mut self, cmd_line: &str) -> Result<()> {
        self.terminal.clear()?;
        let theme = &self.theme;
        self.terminal
            .draw(|frame| draw_flash(frame, cmd_line, theme))?;
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
    vitals: Option<&RepoVitals>,
    initial: usize,
    theme: &Theme,
) -> Result<Option<usize>> {
    let mut cursor = initial.min(items.len() - 1);
    // ListState 跨帧保留滚动偏移,选中项始终保持在视口内
    let mut list = ListState::default();
    loop {
        list.select(Some(cursor));
        terminal.draw(|frame| match vitals {
            Some(v) => draw_rpg_menu(frame, items, cursor, v, theme),
            None => draw_pick(frame, title, items, &mut list, theme),
        })?;
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

/// 状态条的总格数。
const GAUGE_CELLS: usize = 10;

/// 绘制二级选择列表:双线窗框 + 嵌入标题 + 选中行橙底反色,
/// 与主菜单同一套 RPG 视觉;长列表由 [`ListState`] 滚动。
fn draw_pick(
    frame: &mut Frame,
    title: &str,
    items: &[MenuItem],
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
    // 边框 2 + 左右内边距 4 + 选中符号 2 + 余量 2
    let width = panel_width(content_w + 10, false, area);
    let panel_h = (items.len() as u16 + 2)
        .min(area.height.saturating_sub(3))
        .max(3);
    let panel = place_panel(frame, width, panel_h, false, theme);
    hard_shadow(frame, panel, theme);

    let block = rpg_block(title, theme).title_bottom(
        Line::from(Span::styled(
            " J/K 移动 · Enter 确认 · Q 返回 ",
            Style::new().fg(theme.fg_dim),
        ))
        .centered(),
    );

    let selected = list_state.selected().unwrap_or(0);
    let rows: Vec<ListItem<'static>> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let sel = i == selected;
            let mut spans = vec![
                cursor_span(sel, theme),
                Span::styled(
                    format!(
                        "{}{}",
                        item.label,
                        " ".repeat(label_w.saturating_sub(display_width(&item.label)))
                    ),
                    label_style(sel, theme),
                ),
            ];
            if !item.desc.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    item.desc.clone(),
                    Style::new().fg(if sel { theme.hint_fg } else { theme.fg_dim }),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(rows).block(block);

    frame.render_widget(Clear, panel);
    frame.render_stateful_widget(list, panel, list_state);
}

/// 绘制 RPG 风格主菜单页:PINCER 字标、分隔线、状态窗、指令窗、
/// 说明窗与按键提示;高度不足时按「字标 → 状态窗 → 说明窗」顺序降级。
fn draw_rpg_menu(
    frame: &mut Frame,
    items: &[MenuItem],
    cursor: usize,
    vitals: &RepoVitals,
    theme: &Theme,
) {
    let area = frame.area();
    if area.width < 20 || area.height < 5 {
        return;
    }
    let width = 48u16.min(area.width.saturating_sub(4));
    let menu_h = items.len() as u16 + 2;
    // 各段高度:指令窗与按键行常驻,其余按剩余空间取舍(阴影各占 1 行)
    let total = |logo: bool, status: bool, hint: bool| -> u16 {
        menu_h
            + 2
            + if logo { 4 } else { 0 }
            + if status { 7 } else { 0 }
            + if hint { 4 } else { 0 }
    };
    let mut show_logo = true;
    let mut show_status = true;
    let mut show_hint = true;
    if total(show_logo, show_status, show_hint) > area.height {
        show_logo = false;
    }
    if total(show_logo, show_status, show_hint) > area.height {
        show_status = false;
    }
    if total(show_logo, show_status, show_hint) > area.height {
        show_hint = false;
    }
    let mut need = total(show_logo, show_status, show_hint);
    // 高度富余时,字标与分隔线前后各留一行呼吸空隙
    let breathe = u16::from(show_logo && area.height >= need + 2);
    need += breathe * 2;

    let x = area.x + area.width.saturating_sub(width) / 2;
    let mut y = area.y + area.height.saturating_sub(need) / 2;

    if show_logo {
        let art = tinted_logo(theme);
        let logo_w = art.iter().map(Line::width).max().unwrap_or(0) as u16;
        let logo_area = Rect {
            x: area.x + area.width.saturating_sub(logo_w) / 2,
            y,
            width: logo_w,
            height: art.len() as u16,
        };
        y += logo_area.height + breathe;
        frame.render_widget(Paragraph::new(art), logo_area);
        frame.render_widget(
            Paragraph::new(divider_line("◆ 选择你的指令 ◆", width, theme)),
            Rect {
                x,
                y,
                width,
                height: 1,
            },
        );
        y += 1 + breathe;
    }

    if show_status {
        let panel = Rect {
            x,
            y,
            width,
            height: 6,
        };
        y += 7;
        hard_shadow(frame, panel, theme);
        frame.render_widget(Clear, panel);
        let inner = width.saturating_sub(6) as usize;
        frame.render_widget(
            Paragraph::new(status_rows(vitals, inner, theme)).block(rpg_block("状 态", theme)),
            panel,
        );
    }

    {
        let panel = Rect {
            x,
            y,
            width,
            height: menu_h,
        };
        y += menu_h + 1;
        hard_shadow(frame, panel, theme);
        frame.render_widget(Clear, panel);
        let inner = width.saturating_sub(6) as usize;
        let rows: Vec<Line<'static>> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let sel = i == cursor;
                let desc_style = Style::new().fg(if sel { theme.hint_fg } else { theme.fg_dim });
                let gap = inner
                    .saturating_sub(2 + display_width(&item.label) + display_width(&item.desc))
                    .max(1);
                Line::from(vec![
                    cursor_span(sel, theme),
                    Span::styled(item.label.clone(), label_style(sel, theme)),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(item.desc.clone(), desc_style),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(rows).block(rpg_block("指 令", theme)), panel);
    }

    if show_hint {
        let panel = Rect {
            x,
            y,
            width,
            height: 3,
        };
        y += 4;
        hard_shadow(frame, panel, theme);
        frame.render_widget(Clear, panel);
        let inner = width.saturating_sub(6) as usize;
        let item = &items[cursor];
        let hint = if item.hint.is_empty() {
            &item.desc
        } else {
            &item.hint
        };
        let gap = inner.saturating_sub(display_width(hint) + 1).max(1);
        let row = Line::from(vec![
            Span::styled(hint.clone(), Style::new().fg(theme.hint_fg)),
            Span::raw(" ".repeat(gap)),
            Span::styled(
                "▼",
                Style::new()
                    .fg(theme.rpg_accent)
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ]);
        frame.render_widget(Paragraph::new(row).block(rpg_block("", theme)), panel);
    }

    let keys = Line::from(vec![
        keycap(" J/K ", theme),
        Span::styled(" 移动   ", Style::new().fg(theme.fg_dim)),
        keycap(" Enter ", theme),
        Span::styled(" 确认   ", Style::new().fg(theme.fg_dim)),
        keycap(" Q ", theme),
        Span::styled(" 逃跑", Style::new().fg(theme.fg_dim)),
    ])
    .centered();
    frame.render_widget(
        Paragraph::new(keys),
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
    );
}

/// 状态窗四行:分支 / HP(工作区改动)/ MP(贮藏)/ EXP(待推送)。
fn status_rows(vitals: &RepoVitals, inner: usize, theme: &Theme) -> Vec<Line<'static>> {
    let dim = Style::new().fg(theme.fg_dim);
    let hp = GAUGE_CELLS - vitals.changes.min(GAUGE_CELLS);
    let mp = GAUGE_CELLS - vitals.stashes.min(GAUGE_CELLS);
    let exp = vitals.ahead.unwrap_or(0).min(GAUGE_CELLS);
    let exp_note = match vitals.ahead {
        None => "无上游".to_owned(),
        Some(0) => "已同步".to_owned(),
        Some(n) => format!("↑{n} 待推送"),
    };
    vec![
        spread_line(
            vec![
                Span::styled("分支 ", dim),
                Span::styled(
                    vitals.branch.clone(),
                    Style::new()
                        .fg(theme.rpg_frame)
                        .add_modifier(Modifier::BOLD),
                ),
            ],
            vec![Span::styled(
                format!("Lv.{}", vitals.level),
                Style::new().fg(theme.rpg_gold),
            )],
            inner,
        ),
        spread_line(
            gauge_spans("HP  ", hp, theme.rpg_hp, theme),
            vec![Span::styled(format!("改动 ×{}", vitals.changes), dim)],
            inner,
        ),
        spread_line(
            gauge_spans("MP  ", mp, theme.rpg_mp, theme),
            vec![Span::styled(format!("贮藏 ×{}", vitals.stashes), dim)],
            inner,
        ),
        spread_line(
            gauge_spans("EXP ", exp, theme.rpg_exp, theme),
            vec![Span::styled(exp_note, dim)],
            inner,
        ),
    ]
}

/// 标签 + 十格状态条(已填充格 + 空槽格)。
fn gauge_spans(
    label: &str,
    filled: usize,
    color: ratatui::style::Color,
    theme: &Theme,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(label.to_owned(), Style::new().fg(theme.fg_dim)),
        Span::styled("█".repeat(filled), Style::new().fg(color)),
        Span::styled(
            "░".repeat(GAUGE_CELLS - filled),
            Style::new().fg(theme.rpg_gauge_empty),
        ),
    ]
}

/// 面板内两端对齐的一行:左右两组 spans 之间以空隙撑开。
fn spread_line(left: Vec<Span<'static>>, right: Vec<Span<'static>>, inner: usize) -> Line<'static> {
    let used: usize = left
        .iter()
        .chain(right.iter())
        .map(|s| display_width(&s.content))
        .sum();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(inner.saturating_sub(used).max(1))));
    spans.extend(right);
    Line::from(spans)
}

/// 选中行的光标符号(橙色,闪烁)或等宽占位。
fn cursor_span(selected: bool, theme: &Theme) -> Span<'static> {
    if selected {
        Span::styled(
            "▶ ",
            Style::new()
                .fg(theme.rpg_accent)
                .add_modifier(Modifier::SLOW_BLINK),
        )
    } else {
        Span::raw("  ")
    }
}

/// 条目标签样式:选中为橙色加粗,未选中为常规蓝。
fn label_style(selected: bool, theme: &Theme) -> Style {
    if selected {
        Style::new()
            .fg(theme.rpg_accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.blue)
    }
}

/// RPG 双线窗框:米白框线 + 嵌在上框线的标题(空标题则纯框)。
fn rpg_block(title: &str, theme: &Theme) -> Block<'static> {
    let mut block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(theme.rpg_frame))
        .padding(Padding::new(2, 2, 0, 0));
    if !title.trim().is_empty() {
        block = block.title(Span::styled(
            format!(" ◆ {} ", title.trim()),
            Style::new()
                .fg(theme.rpg_frame)
                .add_modifier(Modifier::BOLD),
        ));
    }
    block
}

/// `──── ◆ 文案 ◆ ────` 分隔线,两侧线段等分剩余宽度。
fn divider_line(text: &str, width: u16, theme: &Theme) -> Line<'static> {
    let tw = display_width(text) + 2;
    let side = (width as usize).saturating_sub(tw) / 2;
    Line::from(vec![
        Span::styled("─".repeat(side), Style::new().fg(theme.fg_dim)),
        Span::styled(format!(" {text} "), Style::new().fg(theme.hint_fg)),
        Span::styled(
            "─".repeat((width as usize).saturating_sub(tw + side)),
            Style::new().fg(theme.fg_dim),
        ),
    ])
}

/// 键帽样式的按键标签。
fn keycap(text: &str, theme: &Theme) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::new().fg(theme.keycap_fg).bg(theme.keycap_bg),
    )
}

/// 面板右下一格偏移的硬阴影(RPG 窗口立体感);需在面板内容之前绘制。
fn hard_shadow(frame: &mut Frame, panel: Rect, theme: &Theme) {
    let shadow = Rect {
        x: panel.x + 1,
        y: panel.y + 1,
        width: panel.width,
        height: panel.height,
    }
    .intersection(frame.area());
    frame.render_widget(
        Block::new().style(Style::new().bg(theme.rpg_shadow)),
        shadow,
    );
}

/// 计算面板宽度:按内容自适应;带 logo 的页面加宽到与 logo 齐宽,不超出屏幕。
fn panel_width(content_w: usize, logo: bool, area: Rect) -> u16 {
    let art_w = logo_art().iter().map(Line::width).max().unwrap_or(0) as u16;
    let fit = content_w as u16;
    if logo && art_w > 0 {
        fit.max(art_w + 2)
    } else {
        fit.max(36)
    }
    .min(area.width.saturating_sub(4))
}

/// 垂直居中放置「logo + 面板」组合并绘制 logo,返回面板区域。
///
/// logo 画在面板上方,两者作为整体居中;屏幕放不下时自动隐藏 logo。
/// 逐行内容左缘对齐(整块统一偏移居中),避免行尾透明格差异造成错位。
fn place_panel(frame: &mut Frame, width: u16, panel_h: u16, logo: bool, theme: &Theme) -> Rect {
    let area = frame.area();
    let art = logo_art();
    let logo_w = art.iter().map(Line::width).max().unwrap_or(0) as u16;
    let logo_h = art.len() as u16 + 1;
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
        frame.render_widget(Paragraph::new(tinted_logo(theme)), logo_area);
    }
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: top + if show_logo { logo_h } else { 0 },
        width: width.min(area.width),
        height: panel_h,
    }
}

/// logo 的着色副本:无自带颜色的字符统一着主题 logo 色(纯文本字符画场景)。
fn tinted_logo(theme: &Theme) -> Vec<Line<'static>> {
    logo_art()
        .iter()
        .map(|line| {
            let spans: Vec<Span<'static>> = line
                .spans
                .iter()
                .map(|s| {
                    if s.style.fg.is_none() && s.style.bg.is_none() && !s.content.trim().is_empty()
                    {
                        Span::styled(s.content.clone(), Style::new().fg(theme.logo))
                    } else {
                        s.clone()
                    }
                })
                .collect();
            Line::from(spans)
        })
        .collect()
}

/// 绘制「命令执行中」等待页:字标 + 双线窗框,命令行高亮展示,
/// 页面结构与主菜单一致以减小切换跳变。
fn draw_flash(frame: &mut Frame, cmd_line: &str, theme: &Theme) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let suffix = "  执行中…";
    // 边框 2 + 左右内边距 4 + 两端余量 2
    let width = panel_width(
        display_width(cmd_line) + display_width(suffix) + 8,
        true,
        area,
    );
    let panel = place_panel(frame, width, 3, true, theme);
    hard_shadow(frame, panel, theme);
    let line = Line::from(vec![
        Span::styled(
            cmd_line.to_owned(),
            Style::new()
                .fg(theme.rpg_accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(suffix, Style::new().fg(theme.fg_dim)),
    ])
    .centered();
    frame.render_widget(Clear, panel);
    frame.render_widget(Paragraph::new(line).block(rpg_block("", theme)), panel);
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

/// 绘制居中的消息弹框:双线窗框 + 琥珀色嵌入标题 + 自动换行正文。
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
    hard_shadow(frame, panel, theme);

    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(theme.rpg_frame))
        .padding(Padding::new(2, 2, 1, 1))
        .title(Span::styled(
            format!(" ◆ {} ", title.trim()),
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

    /// CJK 显示宽度估算(面板宽度自适应用);块元素 / 几何符号按 1 列
    #[test]
    fn display_width_counts_cjk_as_two() {
        assert_eq!(display_width("pull"), 4);
        assert_eq!(display_width("拉取"), 4);
        assert_eq!(display_width("a拉b"), 4);
        assert_eq!(display_width("████░░"), 6);
        assert_eq!(display_width("▶ ◆"), 3);
        assert_eq!(display_width("↑2 待推送"), 9);
    }

    /// 两端对齐行:空隙撑满内宽,总宽恰为 inner
    #[test]
    fn spread_line_fills_inner_width() {
        let line = spread_line(
            vec![Span::raw("HP  "), Span::raw("████")],
            vec![Span::raw("改动 ×2")],
            30,
        );
        let total: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
        assert_eq!(total, 30);
    }

    /// 状态条:填充格与空槽格总和恒为 GAUGE_CELLS
    #[test]
    fn gauge_spans_total_cells() {
        let theme = Theme::default();
        for filled in [0, 3, GAUGE_CELLS] {
            let spans = gauge_spans("HP  ", filled, theme.rpg_hp, &theme);
            let cells: usize = spans[1].content.chars().count() + spans[2].content.chars().count();
            assert_eq!(cells, GAUGE_CELLS);
        }
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

    /// 把 TestBackend 渲染缓冲拼成纯文本(逐行)。
    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let area = buf.area();
        let mut text = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    /// RPG 主菜单在 80×24 终端下渲染出全部区块
    #[test]
    fn rpg_menu_renders_all_sections() {
        use ratatui::{Terminal, backend::TestBackend};
        let items = vec![
            MenuItem::new("pull", "拉取远端").with_hint("从远端拉取最新提交,更新当前分支。"),
            MenuItem::new("merge", "合并分支"),
        ];
        let vitals = RepoVitals {
            branch: "main".to_owned(),
            changes: 2,
            stashes: 3,
            ahead: Some(2),
            level: 128,
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| draw_rpg_menu(f, &items, 0, &vitals, &Theme::default()))
            .unwrap();
        let text = buffer_text(terminal.backend().buffer());
        println!("{text}");
        // CJK 在缓冲中占两格(续格为空格),匹配前去掉全部空格
        let flat = text.replace(' ', "");
        assert!(flat.contains("◆状态"), "状态窗标题缺失");
        assert!(flat.contains("◆指令"), "指令窗标题缺失");
        assert!(flat.contains("▶pull"), "选中行光标缺失");
        assert!(flat.contains("Lv.128"), "等级缺失");
        assert!(flat.contains("↑2待推送"), "待推送计数缺失");
        assert!(flat.contains("选择你的指令"), "分隔线文案缺失");
        assert!(flat.contains("从远端拉取最新提交"), "说明窗文案缺失");
        assert!(flat.contains("逃跑"), "按键提示缺失");
    }

    /// 高度不足时按序降级:12 行终端只保留指令窗与按键行
    #[test]
    fn rpg_menu_degrades_on_short_terminal() {
        use ratatui::{Terminal, backend::TestBackend};
        let items = vec![MenuItem::new("pull", "拉取远端")];
        let vitals = RepoVitals {
            branch: "main".to_owned(),
            changes: 0,
            stashes: 0,
            ahead: None,
            level: 1,
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal
            .draw(|f| draw_rpg_menu(f, &items, 0, &vitals, &Theme::default()))
            .unwrap();
        let flat = buffer_text(terminal.backend().buffer()).replace(' ', "");
        assert!(!flat.contains("◆状态"), "矮终端应隐藏状态窗");
        assert!(flat.contains("◆指令"), "指令窗必须保留");
        assert!(flat.contains("▶pull"));
    }

    /// 内嵌 logo 资产能解析出非空字符画
    #[test]
    fn embedded_logo_parses() {
        let art = logo_art();
        assert!(!art.is_empty());
        assert!(art.iter().map(Line::width).max().unwrap_or(0) > 20);
    }
}
