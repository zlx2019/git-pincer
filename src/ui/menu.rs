//! Shared single-choice list overlays for the action menu and branch / commit pickers.
//!
//! [`MenuSession`] keeps nested menus and dialogs in one ratatui session.
//! Page changes clear and redraw without leaving the alternate screen, avoiding flicker.
//! Dropping the session restores the terminal before subsequent Git output or resolution.

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
use crate::i18n::{tr, tr_f};

use super::theme::{Theme, term_color};

/// Main menu logo art, embedded at compile time.
///
/// Supports ANSI true-color pixel art with preserved colors, or plain text art
/// tinted with the theme's logo color during rendering.
const LOGO_ANSI: &str = include_str!("logo.ans");

/// Parse and cache the logo once per process, adapting colors through term_color.
fn logo_art() -> &'static [Line<'static>] {
    static ART: OnceLock<Vec<Line<'static>>> = OnceLock::new();
    ART.get_or_init(|| parse_ansi_art(LOGO_ANSI))
}

/// Parse ANSI art into ratatui lines.
///
/// Supports the logo's SGR subset: `0` reset, `38;2;r;g;b` foreground,
/// and `48;2;r;g;b` background. Unstyled spaces are transparent.
/// Leading and trailing empty rows are trimmed.
fn parse_ansi_art(src: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<(Line<'static>, bool)> = Vec::new();
    for raw in src.lines() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut visible = false;
        let (mut fg, mut bg): (Option<Style>, Option<Style>) = (None, None);
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Collect parameters between `[` and `m`.
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
                // Colored or non-whitespace characters are visible, preserving plain text art.
                if fg.is_some() || bg.is_some() || !c.is_whitespace() {
                    visible = true;
                }
                spans.push(Span::styled(c.to_string(), style));
            }
        }
        lines.push((Line::from(spans), visible));
    }
    // Trim leading and trailing rows with no visible pixels.
    while lines.last().is_some_and(|(_, v)| !v) {
        lines.pop();
    }
    let skip = lines.iter().take_while(|(_, v)| !v).count();
    lines.drain(..skip);
    lines.into_iter().map(|(l, _)| l).collect()
}

/// Apply SGR parameters to the current foreground and background state.
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

/// Menu entry: a command / branch / short-hash label and optional description in two colored columns.
#[derive(Debug, Clone)]
pub(crate) struct MenuItem {
    /// Left-column label used as the selected value
    pub(crate) label: String,
    /// Right-column description; empty to display only the label
    pub(crate) desc: String,
    /// Extended description in the main menu's bottom panel; falls back to desc when empty
    pub(crate) hint: String,
}

impl MenuItem {
    /// Construct an entry.
    pub(crate) fn new(label: impl Into<String>, desc: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            desc: desc.into(),
            hint: String::new(),
        }
    }

    /// Attach an extended description for the information panel.
    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }
}

/// Estimate display width: CJK uses two columns; arrows, box drawing, blocks, and shapes use one.
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

/// Move the cursor with wrapping; `delta` is +1 / -1.
fn wrap_move(cursor: usize, len: usize, delta: isize) -> usize {
    (cursor as isize + delta).rem_euclid(len as isize) as usize
}

/// One continuous menu TUI session sharing a terminal across nested pickers and dialogs.
///
/// Page changes redraw without repeatedly entering and leaving the alternate screen.
/// Drop restores the terminal. [`Self::into_terminal`] transfers ownership to the
/// conflict resolver while preserving the screen without flicker.
pub(crate) struct MenuSession {
    /// Owned terminal; None after transfer, disabling restoration on Drop
    terminal: Option<ratatui::DefaultTerminal>,
    /// UI theme
    theme: Theme,
}

impl MenuSession {
    /// Open a menu TUI session; reject non-interactive terminals.
    pub(crate) fn open(light: bool) -> Result<Self> {
        if !std::io::stdout().is_terminal() {
            anyhow::bail!("{}", tr("common.need_tty_menu"));
        }
        Ok(Self {
            terminal: Some(ratatui::init()),
            theme: Theme::select(light),
        })
    }

    /// Transfer terminal ownership to the resolver without leaving the alternate screen.
    /// Drop will no longer restore the terminal; the recipient assumes that
    /// responsibility.
    pub(crate) fn into_terminal(mut self) -> Result<ratatui::DefaultTerminal> {
        self.terminal
            .take()
            .ok_or_else(|| anyhow::anyhow!("menu session already handed over the terminal"))
    }

    /// Run a picker and return the selected index, or None for cancellation or an empty list.
    /// With `vitals`, draw the full RPG menu with logo, stats, and description;
    /// otherwise draw a double-bordered list. `initial` sets the starting cursor.
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
        let Some(terminal) = self.terminal.as_mut() else {
            anyhow::bail!("menu session already handed over the terminal");
        };
        // Force a full redraw to remove remnants from differently sized previous panels.
        terminal.clear()?;
        pick_loop(terminal, title, items, vitals, initial, &self.theme)
    }

    /// Show a message dialog, such as a failure reason; any key closes it.
    pub(crate) fn notice(&mut self, title: &str, body: &str) -> Result<()> {
        let Some(terminal) = self.terminal.as_mut() else {
            anyhow::bail!("menu session already handed over the terminal");
        };
        terminal.clear()?;
        notice_loop(terminal, title, body, &self.theme)
    }

    /// Draw one running-page frame and return immediately without waiting for input,
    /// providing feedback during subsequent blocking operations such as network Git commands.
    pub(crate) fn flash(&mut self, cmd_line: &str) -> Result<()> {
        let Some(terminal) = self.terminal.as_mut() else {
            anyhow::bail!("menu session already handed over the terminal");
        };
        terminal.clear()?;
        let theme = &self.theme;
        terminal.draw(|frame| draw_flash(frame, cmd_line, theme))?;
        Ok(())
    }
}

impl Drop for MenuSession {
    fn drop(&mut self) {
        if self.terminal.is_some() {
            ratatui::restore();
        }
    }
}

/// Picker event loop: draw, read a key, then move / confirm / cancel.
fn pick_loop(
    terminal: &mut ratatui::DefaultTerminal,
    title: &str,
    items: &[MenuItem],
    vitals: Option<&RepoVitals>,
    initial: usize,
    theme: &Theme,
) -> Result<Option<usize>> {
    let mut cursor = initial.min(items.len() - 1);
    // ListState preserves scroll offsets across frames, keeping the selection visible.
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
            // Ctrl+C cancels as expected; raw mode does not generate SIGINT.
            KeyCode::Char('c') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                return Ok(None);
            }
            _ => {}
        }
    }
}

/// Total cells in a status gauge.
const GAUGE_CELLS: usize = 10;

/// Draw a target picker with double borders, an embedded title, and orange selection,
/// matching the RPG menu. [`ListState`] handles scrolling for long lists.
fn draw_pick(
    frame: &mut Frame,
    title: &str,
    items: &[MenuItem],
    list_state: &mut ListState,
    theme: &Theme,
) {
    let area = frame.area();
    // Align labels to the longest entry and fit panel width to content within the screen.
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
    // Borders 2 + horizontal padding 4 + selection marker 2 + margin 2.
    let width = panel_width(content_w + 10, false, area);
    let panel_h = (items.len() as u16 + 2)
        .min(area.height.saturating_sub(3))
        .max(3);
    let panel = place_panel(frame, width, panel_h, false, theme);

    let block = rpg_block(title, theme).title_bottom(
        Line::from(Span::styled(
            format!(" {} ", tr("menu.list_keys")),
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

/// Draw the RPG menu: PINCER logo, divider, stats, commands, description, and key hints.
/// As height shrinks, hide the logo, then stats, then description.
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
    // Always retain commands and key hints; fit other sections with one row between panels.
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
    // Add a row around the logo and divider when height permits.
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
            Paragraph::new(divider_line(
                &format!("◆ {} ◆", tr("menu.divider")),
                width,
                theme,
            )),
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
        frame.render_widget(Clear, panel);
        let inner = width.saturating_sub(6) as usize;
        frame.render_widget(
            Paragraph::new(status_rows(vitals, inner, theme))
                .block(rpg_block(tr("menu.status_title"), theme)),
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
        frame.render_widget(
            Paragraph::new(rows).block(rpg_block(tr("menu.command_title"), theme)),
            panel,
        );
    }

    if show_hint {
        let panel = Rect {
            x,
            y,
            width,
            height: 3,
        };
        y += 4;
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
        Span::styled(
            format!(" {}   ", tr("menu.key_move")),
            Style::new().fg(theme.fg_dim),
        ),
        keycap(" Enter ", theme),
        Span::styled(
            format!(" {}   ", tr("menu.key_confirm")),
            Style::new().fg(theme.fg_dim),
        ),
        keycap(" Q ", theme),
        Span::styled(
            format!(" {}", tr("menu.key_flee")),
            Style::new().fg(theme.fg_dim),
        ),
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

/// Four status rows: branch / HP (worktree changes) / MP (stashes) / EXP (commits ahead).
fn status_rows(vitals: &RepoVitals, inner: usize, theme: &Theme) -> Vec<Line<'static>> {
    let dim = Style::new().fg(theme.fg_dim);
    let hp = GAUGE_CELLS - vitals.changes.min(GAUGE_CELLS);
    let mp = GAUGE_CELLS - vitals.stashes.min(GAUGE_CELLS);
    let exp = vitals.ahead.unwrap_or(0).min(GAUGE_CELLS);
    let exp_note = match vitals.ahead {
        None => tr("menu.no_upstream").to_owned(),
        Some(0) => tr("menu.synced").to_owned(),
        Some(n) => tr_f("menu.ahead", &[("n", &n.to_string())]),
    };
    // Align labels; the branch label may be wider than HP / MP / EXP.
    let branch_label = tr("menu.branch");
    let label_w = display_width(branch_label).max(3) + 1;
    let pad = |label: &str| format!("{label}{}", " ".repeat(label_w - display_width(label)));
    vec![
        spread_line(
            vec![
                Span::styled(pad(branch_label), dim),
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
            gauge_spans(&pad("HP"), hp, theme.rpg_hp, theme),
            vec![Span::styled(
                tr_f("menu.changes", &[("n", &vitals.changes.to_string())]),
                dim,
            )],
            inner,
        ),
        spread_line(
            gauge_spans(&pad("MP"), mp, theme.rpg_mp, theme),
            vec![Span::styled(
                tr_f("menu.stashes", &[("n", &vitals.stashes.to_string())]),
                dim,
            )],
            inner,
        ),
        spread_line(
            gauge_spans(&pad("EXP"), exp, theme.rpg_exp, theme),
            vec![Span::styled(exp_note, dim)],
            inner,
        ),
    ]
}

/// Label and ten-cell gauge with filled and empty slots.
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

/// Justify a panel row by adding space between the left and right span groups.
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

/// Orange blinking cursor for a selected row, or an equal-width placeholder.
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

/// Entry label style: bold orange when selected, regular blue otherwise.
fn label_style(selected: bool, theme: &Theme) -> Style {
    if selected {
        Style::new()
            .fg(theme.rpg_accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.blue)
    }
}

/// RPG double border in off-white, with a title in the top edge if nonempty.
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

/// Decorated text divider with the remaining width split equally between side rules.
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

/// Keycap-style key label.
fn keycap(text: &str, theme: &Theme) -> Span<'static> {
    Span::styled(
        text.to_owned(),
        Style::new().fg(theme.keycap_fg).bg(theme.keycap_bg),
    )
}

/// Fit panel width to content and any logo, constrained to the screen.
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

/// Vertically center the logo and panel, draw the logo, and return the panel area.
///
/// Place the logo above the panel and center them together; hide the logo if needed.
/// Align row contents to one shared left edge to avoid shifts from trailing transparent cells.
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

/// Tinted logo copy, applying the theme logo color to characters without embedded colors.
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

/// Draw a running page with logo, double border, and highlighted command,
/// matching the main menu structure to minimize visual shifts.
fn draw_flash(frame: &mut Frame, cmd_line: &str, theme: &Theme) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let suffix = format!("  {}", tr("menu.running"));
    // Borders 2 + horizontal padding 4 + edge margins 2.
    let width = panel_width(
        display_width(cmd_line) + display_width(&suffix) + 8,
        true,
        area,
    );
    let panel = place_panel(frame, width, 3, true, theme);
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

/// Dialog loop: draw once and wait for any key to close.
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

/// Draw a centered message dialog with double borders, an amber title, and wrapped text.
fn draw_notice(frame: &mut Frame, title: &str, body: &str, theme: &Theme) {
    let area = frame.area();
    let width = 64u16.min(area.width.saturating_sub(4)).max(20);
    let inner_w = width.saturating_sub(6) as usize;
    // Estimate wrapped line count to determine dialog height.
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
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(theme.rpg_frame))
        .padding(Padding::new(2, 2, 1, 1))
        .title(Span::styled(
            format!(" ◆ {} ", title.trim()),
            Style::new().fg(theme.amber).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                format!(" {} ", tr("common.close_any_key")),
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

    /// Cursor movement wraps at list boundaries.
    #[test]
    fn wrap_move_cycles() {
        assert_eq!(wrap_move(0, 5, -1), 4); // Moving up from the top wraps to the bottom.
        assert_eq!(wrap_move(4, 5, 1), 0); // Moving down from the bottom wraps to the top.
        assert_eq!(wrap_move(2, 5, 1), 3);
    }

    /// CJK width estimates for panel sizing; blocks and geometric symbols use one column.
    #[test]
    fn display_width_counts_cjk_as_two() {
        assert_eq!(display_width("pull"), 4);
        assert_eq!(display_width("拉取"), 4);
        assert_eq!(display_width("a拉b"), 4);
        assert_eq!(display_width("████░░"), 6);
        assert_eq!(display_width("▶ ◆"), 3);
        assert_eq!(display_width("↑2 待推送"), 9);
    }

    /// Justified rows fill exactly the available inner width.
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

    /// Filled and empty gauge slots always total GAUGE_CELLS.
    #[test]
    fn gauge_spans_total_cells() {
        let theme = Theme::default();
        for filled in [0, 3, GAUGE_CELLS] {
            let spans = gauge_spans("HP  ", filled, theme.rpg_hp, &theme);
            let cells: usize = spans[1].content.chars().count() + spans[2].content.chars().count();
            assert_eq!(cells, GAUGE_CELLS);
        }
    }

    /// Parse ANSI half-block art: color state, transparent cells, and trailing empty-row trimming.
    #[test]
    fn parses_ansi_half_block_art() {
        let src = "\x1b[0m \x1b[38;2;10;20;30m\x1b[48;2;40;50;60m▀\x1b[0m \n\x1b[0m \x1b[0m ";
        let lines = parse_ansi_art(src);
        assert_eq!(lines.len(), 1, "trailing transparent rows must be trimmed");
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 3); // Transparent cell + pixel + transparent cell.
        assert_eq!(spans[1].content, "▀");
        assert!(spans[1].style.fg.is_some());
        assert!(spans[1].style.bg.is_some());
        assert!(
            spans[0].style.fg.is_none(),
            "transparent cells must be unstyled"
        );
    }

    /// Plain text art without ANSI colors must not be trimmed away.
    #[test]
    fn plain_text_art_is_not_trimmed() {
        let lines = parse_ansi_art("ABC\n\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans.len(), 3);
    }

    /// Convert the TestBackend render buffer into plain text, row by row.
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

    /// The RPG menu renders all sections in an 80x24 terminal.
    #[test]
    fn rpg_menu_renders_all_sections() {
        use ratatui::{Terminal, backend::TestBackend};
        let items = vec![
            MenuItem::new("pull", "Pull from remote")
                .with_hint("Fetch the latest commits and update the current branch."),
            MenuItem::new("merge", "Merge a branch"),
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
        // Remove layout spaces before matching rendered text.
        let flat = text.replace(' ', "");
        assert!(flat.contains("◆STATUS"), "status title missing");
        assert!(flat.contains("◆COMMAND"), "command title missing");
        assert!(flat.contains("▶pull"), "selection cursor missing");
        assert!(flat.contains("Lv.128"), "level missing");
        assert!(flat.contains("↑2topush"), "commits-ahead count missing");
        assert!(flat.contains("ChooseYourCommand"), "divider text missing");
        assert!(
            flat.contains("Fetchthelatestcommits"),
            "description text missing"
        );
        assert!(flat.contains("Flee"), "key hint missing");
    }

    /// A 12-row terminal retains only commands and key hints as sections are hidden in order.
    #[test]
    fn rpg_menu_degrades_on_short_terminal() {
        use ratatui::{Terminal, backend::TestBackend};
        let items = vec![MenuItem::new("pull", "Pull from remote")];
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
        assert!(
            !flat.contains("◆STATUS"),
            "short terminals must hide the status panel"
        );
        assert!(
            flat.contains("◆COMMAND"),
            "command panel must remain visible"
        );
        assert!(flat.contains("▶pull"));
    }

    /// The embedded logo asset parses into nonempty art.
    #[test]
    fn embedded_logo_parses() {
        let art = logo_art();
        assert!(!art.is_empty());
        assert!(art.iter().map(Line::width).max().unwrap_or(0) > 20);
    }
}
