//! Central key-binding table: dispatch, the hint bar and the help overlay
//! all derive from one effective table, so a key can never drift out of sync
//! between behavior and documentation.
//!
//! The effective table = built-in defaults + `[keys]` overrides from the user
//! config, frozen once per process via [`init`]. Key labels shown in the UI
//! are derived from the actual key codes, so overridden keys are displayed
//! correctly everywhere for free.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use crate::i18n::tr_f;

/// 冲突解决会话内的可绑定动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    /// 取用本地侧
    TakeLocal,
    /// 取用远端侧
    TakeRemote,
    /// 忽略当前块仍待处理的侧
    IgnoreChunk,
    /// 撤销当前块的决定
    UndoChunk,
    /// 撤销当前文件的全部决定
    UndoFile,
    /// 用 $EDITOR 编辑当前块
    EditChunk,
    /// 一键取用所有非冲突改动
    ApplyNonConflict,
    /// 移到下一个改动块
    NextChange,
    /// 移到上一个改动块
    PrevChange,
    /// 跳到下一个未解决冲突
    NextConflict,
    /// 跳到上一个未解决冲突
    PrevConflict,
    /// 复制当前块结果
    CopyChunk,
    /// 复制整个文件结果
    CopyFile,
    /// 复制当前块本地侧
    CopyLocal,
    /// 复制当前块远端侧
    CopyRemote,
    /// 写盘当前文件
    WriteFile,
    /// 切换到下一个文件
    NextFile,
    /// 折叠 / 展开稳定区
    ToggleFold,
    /// 退出会话
    Quit,
    /// 打开帮助浮层
    Help,
}

/// 配置文件 `[keys]` 使用的动作名(kebab-case)。
static ACTION_NAMES: &[(&str, Action)] = &[
    ("take-local", Action::TakeLocal),
    ("take-remote", Action::TakeRemote),
    ("ignore", Action::IgnoreChunk),
    ("undo", Action::UndoChunk),
    ("undo-file", Action::UndoFile),
    ("edit", Action::EditChunk),
    ("apply-all", Action::ApplyNonConflict),
    ("next-change", Action::NextChange),
    ("prev-change", Action::PrevChange),
    ("next-conflict", Action::NextConflict),
    ("prev-conflict", Action::PrevConflict),
    ("copy-chunk", Action::CopyChunk),
    ("copy-file", Action::CopyFile),
    ("copy-local", Action::CopyLocal),
    ("copy-remote", Action::CopyRemote),
    ("write", Action::WriteFile),
    ("next-file", Action::NextFile),
    ("fold", Action::ToggleFold),
    ("quit", Action::Quit),
    ("help", Action::Help),
];

/// 一组默认按键绑定:展示上是一行(如 `u / U`),分发上可含多个键位。
struct Binding {
    /// 组的标识动作(布局列表以此引用)
    id: Action,
    /// 键位 → 动作(如 u 撤销块、U 撤销整个文件同属一组)
    keys: &'static [(KeyCode, Action)],
    /// 提示条短文案的 i18n key;None 表示该组不进提示条
    hint: Option<&'static str>,
    /// 帮助浮层文案的 i18n key
    help: &'static str,
}

/// 全部默认绑定(单一事实来源;标签由键位派生,不在此硬编码)。
static BINDINGS: &[Binding] = &[
    Binding {
        id: Action::TakeLocal,
        keys: &[
            (KeyCode::Char('h'), Action::TakeLocal),
            (KeyCode::Left, Action::TakeLocal),
        ],
        hint: Some("ui.hint_left"),
        help: "ui.help_left",
    },
    Binding {
        id: Action::TakeRemote,
        keys: &[
            (KeyCode::Char('l'), Action::TakeRemote),
            (KeyCode::Right, Action::TakeRemote),
        ],
        hint: Some("ui.hint_right"),
        help: "ui.help_right",
    },
    Binding {
        id: Action::IgnoreChunk,
        keys: &[(KeyCode::Char('x'), Action::IgnoreChunk)],
        hint: Some("ui.hint_ignore"),
        help: "ui.help_ignore",
    },
    Binding {
        id: Action::UndoChunk,
        keys: &[
            (KeyCode::Char('u'), Action::UndoChunk),
            (KeyCode::Char('U'), Action::UndoFile),
        ],
        hint: Some("ui.hint_undo"),
        help: "ui.help_undo",
    },
    Binding {
        id: Action::EditChunk,
        keys: &[(KeyCode::Char('e'), Action::EditChunk)],
        hint: Some("ui.hint_edit"),
        help: "ui.help_edit",
    },
    Binding {
        id: Action::ApplyNonConflict,
        keys: &[(KeyCode::Char('a'), Action::ApplyNonConflict)],
        hint: None,
        help: "ui.help_apply",
    },
    Binding {
        id: Action::WriteFile,
        keys: &[(KeyCode::Char('w'), Action::WriteFile)],
        hint: Some("ui.hint_write"),
        help: "ui.help_write",
    },
    Binding {
        id: Action::Quit,
        keys: &[(KeyCode::Char('q'), Action::Quit)],
        hint: Some("ui.hint_quit"),
        help: "ui.help_quit",
    },
    Binding {
        id: Action::NextChange,
        keys: &[
            (KeyCode::Char('j'), Action::NextChange),
            (KeyCode::Down, Action::NextChange),
            (KeyCode::Char('k'), Action::PrevChange),
            (KeyCode::Up, Action::PrevChange),
        ],
        hint: None,
        help: "ui.help_move",
    },
    Binding {
        id: Action::NextConflict,
        keys: &[
            (KeyCode::Char('n'), Action::NextConflict),
            (KeyCode::Char('p'), Action::PrevConflict),
        ],
        hint: Some("ui.hint_conflict"),
        help: "ui.help_jump",
    },
    Binding {
        id: Action::NextFile,
        keys: &[(KeyCode::Tab, Action::NextFile)],
        hint: Some("ui.hint_file"),
        help: "ui.help_tab",
    },
    Binding {
        id: Action::ToggleFold,
        keys: &[(KeyCode::Char('z'), Action::ToggleFold)],
        hint: Some("ui.hint_fold"),
        help: "ui.help_fold",
    },
    Binding {
        id: Action::CopyChunk,
        keys: &[(KeyCode::Char('y'), Action::CopyChunk)],
        hint: None,
        help: "ui.help_copy_chunk",
    },
    Binding {
        id: Action::CopyFile,
        keys: &[(KeyCode::Char('Y'), Action::CopyFile)],
        hint: None,
        help: "ui.help_copy_file",
    },
    Binding {
        id: Action::CopyLocal,
        keys: &[
            (KeyCode::Char('H'), Action::CopyLocal),
            (KeyCode::Char('L'), Action::CopyRemote),
        ],
        hint: None,
        help: "ui.help_copy_sides",
    },
    Binding {
        id: Action::Help,
        keys: &[(KeyCode::Char('?'), Action::Help)],
        hint: Some("ui.hint_help"),
        help: "ui.help_help",
    },
];

/// 提示条的展示顺序(引用绑定组的标识动作)。
static HINT_LAYOUT: &[Action] = &[
    Action::TakeLocal,
    Action::TakeRemote,
    Action::IgnoreChunk,
    Action::UndoChunk,
    Action::EditChunk,
    Action::NextConflict,
    Action::WriteFile,
    Action::NextFile,
    Action::ToggleFold,
    Action::Quit,
    Action::Help,
];

/// 帮助浮层左栏的展示顺序。
static HELP_LEFT: &[Action] = &[
    Action::TakeLocal,
    Action::TakeRemote,
    Action::IgnoreChunk,
    Action::UndoChunk,
    Action::EditChunk,
    Action::ApplyNonConflict,
    Action::WriteFile,
    Action::Quit,
];

/// 帮助浮层右栏的展示顺序。
static HELP_RIGHT: &[Action] = &[
    Action::NextChange,
    Action::NextConflict,
    Action::NextFile,
    Action::ToggleFold,
    Action::CopyChunk,
    Action::CopyFile,
    Action::CopyLocal,
    Action::Help,
];

/// 生效的绑定组:默认表与 `[keys]` 覆盖的合成结果,标签由键位派生。
#[derive(Debug)]
struct EffectiveBinding {
    /// 组的标识动作
    id: Action,
    /// 键位(含修饰键)→ 动作
    keys: Vec<(KeyCode, KeyModifiers, Action)>,
    /// 帮助浮层的按键标签(如 `u / U`)
    label: String,
    /// 提示条的紧凑按键标签(如 `u/U`)
    hint_label: String,
    /// 提示条短文案的 i18n key
    hint: Option<&'static str>,
    /// 帮助浮层文案的 i18n key
    help: &'static str,
}

/// 进程内的生效绑定表(init 冻结;未显式 init 时惰性取默认)。
static EFFECTIVE: OnceLock<Vec<EffectiveBinding>> = OnceLock::new();

/// 应用配置的 `[keys]` 覆盖并冻结生效表(进程内仅首次调用生效,
/// 应在进入任何 TUI 之前调用)。
pub(crate) fn init(overrides: &HashMap<String, String>) -> Result<()> {
    let table = build(overrides)?;
    let _ = EFFECTIVE.set(table);
    Ok(())
}

/// 取生效表;未 init 时按无覆盖构建(默认构建不会失败)。
fn effective() -> &'static [EffectiveBinding] {
    EFFECTIVE.get_or_init(|| build(&HashMap::new()).unwrap_or_default())
}

/// 由默认表与覆盖合成生效表:被覆盖的动作以配置键替换其全部默认键位,
/// 并做全表键位冲突检测。
fn build(overrides: &HashMap<String, String>) -> Result<Vec<EffectiveBinding>> {
    let mut parsed: HashMap<Action, (KeyCode, KeyModifiers)> = HashMap::new();
    for (name, desc) in overrides {
        let Some(&(_, action)) = ACTION_NAMES.iter().find(|(n, _)| n == name) else {
            let list: Vec<&str> = ACTION_NAMES.iter().map(|(n, _)| *n).collect();
            bail!(
                "{}",
                tr_f(
                    "config.bad_action",
                    &[("name", name), ("list", &list.join(", "))],
                )
            );
        };
        let Some(key) = parse_key(desc) else {
            bail!(
                "{}",
                tr_f("config.bad_key", &[("name", name), ("key", desc)])
            );
        };
        parsed.insert(action, key);
    }

    let mut table = Vec::with_capacity(BINDINGS.len());
    for binding in BINDINGS {
        let mut keys: Vec<(KeyCode, KeyModifiers, Action)> = Vec::new();
        let mut replaced: Vec<Action> = Vec::new();
        for (code, action) in binding.keys {
            match parsed.get(action) {
                // 覆盖:该动作的全部默认键位替换为配置键(只插入一次)
                Some(&(new_code, new_mods)) => {
                    if !replaced.contains(action) {
                        keys.push((new_code, new_mods, *action));
                        replaced.push(*action);
                    }
                }
                None => keys.push((*code, KeyModifiers::NONE, *action)),
            }
        }
        let (label, hint_label) = derive_labels(&keys);
        table.push(EffectiveBinding {
            id: binding.id,
            keys,
            label,
            hint_label,
            hint: binding.hint,
            help: binding.help,
        });
    }

    let mut seen: HashSet<(KeyCode, KeyModifiers)> = HashSet::new();
    for (code, mods, _) in table.iter().flat_map(|b| &b.keys) {
        if !seen.insert((*code, *mods)) {
            bail!(
                "{}",
                tr_f(
                    "config.key_conflict",
                    &[("key", &key_display(*code, *mods, false))],
                )
            );
        }
    }
    Ok(table)
}

/// 解析键位描述:单字符(区分大小写)、命名键(left / tab / f5 / space 等,
/// 不区分大小写),可带 `ctrl+` / `alt+` / `shift+` 前缀;
/// `shift+字母` 归一化为大写字符。无法解析返回 None。
fn parse_key(desc: &str) -> Option<(KeyCode, KeyModifiers)> {
    let parts: Vec<&str> = desc.split('+').map(str::trim).collect();
    let (mod_parts, key_part) = parts.split_at(parts.len() - 1);
    let name = *key_part.first()?;
    if name.is_empty() {
        return None;
    }

    let mut mods = KeyModifiers::NONE;
    for part in mod_parts {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "alt" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            _ => return None,
        }
    }

    let lower = name.to_ascii_lowercase();
    let mut code = match lower.as_str() {
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "tab" => KeyCode::Tab,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        f if f.len() >= 2 && f.starts_with('f') && f[1..].chars().all(|c| c.is_ascii_digit()) => {
            let n: u8 = f[1..].parse().ok()?;
            if !(1..=12).contains(&n) {
                return None;
            }
            KeyCode::F(n)
        }
        _ => {
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            KeyCode::Char(c)
        }
    };
    // shift+字母 → 大写字符;Char 键的 SHIFT 修饰无独立意义,统一并入字符
    if mods.contains(KeyModifiers::SHIFT)
        && let KeyCode::Char(c) = code
    {
        code = KeyCode::Char(c.to_ascii_uppercase());
        mods -= KeyModifiers::SHIFT;
    }
    Some((code, mods))
}

/// 单个键位的展示名;`compact` 用于提示条(Tab 显示为 ⇥)。
fn key_display(code: KeyCode, mods: KeyModifiers, compact: bool) -> String {
    let base = match code {
        KeyCode::Char(' ') => "Space".to_owned(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Left => "←".to_owned(),
        KeyCode::Right => "→".to_owned(),
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::Tab => if compact { "⇥" } else { "Tab" }.to_owned(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    let mut out = String::new();
    if mods.contains(KeyModifiers::CONTROL) {
        out.push_str("ctrl+");
    }
    if mods.contains(KeyModifiers::ALT) {
        out.push_str("alt+");
    }
    out.push_str(&base);
    out
}

/// 从组的键位派生 (帮助标签, 提示条紧凑标签):
/// 单动作组列出全部键位(如 `h / ←`),多动作组各取首键(如 `u / U`)。
fn derive_labels(keys: &[(KeyCode, KeyModifiers, Action)]) -> (String, String) {
    let mut actions: Vec<Action> = Vec::new();
    for (_, _, action) in keys {
        if !actions.contains(action) {
            actions.push(*action);
        }
    }
    let first_key = |action: Action, compact: bool| -> Option<String> {
        keys.iter()
            .find(|(_, _, a)| *a == action)
            .map(|(c, m, _)| key_display(*c, *m, compact))
    };
    let label = if actions.len() == 1 {
        keys.iter()
            .map(|(c, m, _)| key_display(*c, *m, false))
            .collect::<Vec<_>>()
            .join(" / ")
    } else {
        actions
            .iter()
            .filter_map(|a| first_key(*a, false))
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let hint_label = actions
        .iter()
        .filter_map(|a| first_key(*a, true))
        .collect::<Vec<_>>()
        .join("/");
    (label, hint_label)
}

/// 把按键事件解析为动作;未绑定返回 None。
/// Char 键忽略 SHIFT 修饰(大小写已编码进字符本身)。
pub(crate) fn action_for(code: KeyCode, mods: KeyModifiers) -> Option<Action> {
    action_in(effective(), code, mods)
}

/// 在指定表中查找按键(action_for 的纯逻辑部分,便于测试覆盖表)。
fn action_in(table: &[EffectiveBinding], code: KeyCode, mods: KeyModifiers) -> Option<Action> {
    let mods = match code {
        KeyCode::Char(_) => mods - KeyModifiers::SHIFT,
        _ => mods,
    };
    table
        .iter()
        .flat_map(|b| &b.keys)
        .find(|(key, m, _)| *key == code && *m == mods)
        .map(|(_, _, action)| *action)
}

/// 按标识动作取生效绑定组(布局列表引用用)。
fn group(id: Action) -> Option<&'static EffectiveBinding> {
    effective().iter().find(|b| b.id == id)
}

/// 提示条条目:(紧凑按键标签, 文案 i18n key),按展示顺序。
pub(crate) fn hint_entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    HINT_LAYOUT.iter().filter_map(|id| {
        let binding = group(*id)?;
        Some((binding.hint_label.as_str(), binding.hint?))
    })
}

/// 帮助条目:(按键标签, 文案 i18n key)。
pub(crate) type HelpEntry = (&'static str, &'static str);

/// 帮助浮层某一栏的条目。
fn help_entries(layout: &'static [Action]) -> Vec<HelpEntry> {
    layout
        .iter()
        .filter_map(|id| group(*id).map(|b| (b.label.as_str(), b.help)))
        .collect()
}

/// 帮助浮层的左 / 右两栏条目。
pub(crate) fn help_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    (help_entries(HELP_LEFT), help_entries(HELP_RIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无覆盖构建的默认表
    fn default_table() -> Vec<EffectiveBinding> {
        build(&HashMap::new()).unwrap()
    }

    /// 常用键解析到预期动作;未绑定键返回 None;Char 键忽略 SHIFT
    #[test]
    fn keys_resolve_to_expected_actions() {
        let t = default_table();
        let f = |code| action_in(&t, code, KeyModifiers::NONE);
        assert_eq!(f(KeyCode::Char('h')), Some(Action::TakeLocal));
        assert_eq!(f(KeyCode::Left), Some(Action::TakeLocal));
        assert_eq!(f(KeyCode::Char('U')), Some(Action::UndoFile));
        assert_eq!(f(KeyCode::Char('k')), Some(Action::PrevChange));
        assert_eq!(f(KeyCode::Tab), Some(Action::NextFile));
        assert_eq!(f(KeyCode::Char('?')), Some(Action::Help));
        assert_eq!(f(KeyCode::Char('0')), None);
        assert_eq!(f(KeyCode::Esc), None);
        // 终端为大写字符附带的 SHIFT 修饰不影响匹配
        assert_eq!(
            action_in(&t, KeyCode::Char('U'), KeyModifiers::SHIFT),
            Some(Action::UndoFile)
        );
        // 非 Char 键的修饰键需精确匹配
        assert_eq!(action_in(&t, KeyCode::Left, KeyModifiers::CONTROL), None);
    }

    /// 默认表派生的标签与既有界面完全一致(视觉回归守卫)
    #[test]
    fn default_labels_match_previous_ui() {
        let expected: [(&str, &str); 16] = [
            ("h / ←", "h"),
            ("l / →", "l"),
            ("x", "x"),
            ("u / U", "u/U"),
            ("e", "e"),
            ("a", "a"),
            ("w", "w"),
            ("q", "q"),
            ("j / k", "j/k"),
            ("n / p", "n/p"),
            ("Tab", "⇥"),
            ("z", "z"),
            ("y", "y"),
            ("Y", "Y"),
            ("H / L", "H/L"),
            ("?", "?"),
        ];
        let table = default_table();
        assert_eq!(table.len(), expected.len());
        for (binding, (label, hint_label)) in table.iter().zip(expected) {
            assert_eq!(binding.label, label, "{:?} 帮助标签漂移", binding.id);
            assert_eq!(
                binding.hint_label, hint_label,
                "{:?} 提示条标签漂移",
                binding.id
            );
        }
    }

    /// 键位描述解析:命名键 / 修饰键 / shift 归一化 / 非法输入
    #[test]
    fn parse_key_descriptions() {
        assert_eq!(
            parse_key("h"),
            Some((KeyCode::Char('h'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("U"),
            Some((KeyCode::Char('U'), KeyModifiers::NONE))
        );
        assert_eq!(parse_key("LEFT"), Some((KeyCode::Left, KeyModifiers::NONE)));
        assert_eq!(
            parse_key("ctrl+s"),
            Some((KeyCode::Char('s'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("alt+enter"),
            Some((KeyCode::Enter, KeyModifiers::ALT))
        );
        assert_eq!(
            parse_key("shift+u"),
            Some((KeyCode::Char('U'), KeyModifiers::NONE))
        );
        assert_eq!(parse_key("f5"), Some((KeyCode::F(5), KeyModifiers::NONE)));
        assert_eq!(
            parse_key("space"),
            Some((KeyCode::Char(' '), KeyModifiers::NONE))
        );
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("ctrl+"), None);
        assert_eq!(parse_key("foo"), None);
        assert_eq!(parse_key("f13"), None);
        assert_eq!(parse_key("meta+x"), None);
    }

    /// [keys] 覆盖:替换动作的全部默认键位,标签同步更新
    #[test]
    fn overrides_replace_defaults_and_labels() {
        let overrides = HashMap::from([
            ("take-local".to_owned(), "o".to_owned()),
            ("write".to_owned(), "ctrl+s".to_owned()),
        ]);
        let table = build(&overrides).unwrap();
        assert_eq!(
            action_in(&table, KeyCode::Char('o'), KeyModifiers::NONE),
            Some(Action::TakeLocal)
        );
        // 默认键位 h 与 ← 均已被替换
        assert_eq!(
            action_in(&table, KeyCode::Char('h'), KeyModifiers::NONE),
            None
        );
        assert_eq!(action_in(&table, KeyCode::Left, KeyModifiers::NONE), None);
        assert_eq!(
            action_in(&table, KeyCode::Char('s'), KeyModifiers::CONTROL),
            Some(Action::WriteFile)
        );
        let take_local = table.iter().find(|b| b.id == Action::TakeLocal).unwrap();
        assert_eq!(
            (take_local.label.as_str(), take_local.hint_label.as_str()),
            ("o", "o")
        );
        let write = table.iter().find(|b| b.id == Action::WriteFile).unwrap();
        assert_eq!(write.label, "ctrl+s");
    }

    /// 组内单个动作覆盖:另一动作保留默认键,组标签合并展示
    #[test]
    fn partial_group_override_keeps_sibling() {
        let overrides = HashMap::from([("undo".to_owned(), "ctrl+z".to_owned())]);
        let table = build(&overrides).unwrap();
        assert_eq!(
            action_in(&table, KeyCode::Char('z'), KeyModifiers::CONTROL),
            Some(Action::UndoChunk)
        );
        assert_eq!(
            action_in(&table, KeyCode::Char('U'), KeyModifiers::NONE),
            Some(Action::UndoFile)
        );
        let undo = table.iter().find(|b| b.id == Action::UndoChunk).unwrap();
        assert_eq!(undo.label, "ctrl+z / U");
    }

    /// 覆盖冲突与非法输入给出可读错误
    #[test]
    fn override_errors_are_readable() {
        let bad_action = HashMap::from([("no-such".to_owned(), "o".to_owned())]);
        let err = build(&bad_action).unwrap_err().to_string();
        assert!(err.contains("no-such") && err.contains("take-local"));

        let bad_key = HashMap::from([("write".to_owned(), "ctrl+".to_owned())]);
        let err = build(&bad_key).unwrap_err().to_string();
        assert!(err.contains("ctrl+") && err.contains("write"));

        // 覆盖键撞上其他动作的默认键
        let conflict = HashMap::from([("write".to_owned(), "h".to_owned())]);
        let err = build(&conflict).unwrap_err().to_string();
        assert!(err.contains('h'));
    }

    /// 布局列表引用的组必须存在;提示条组必须携带提示文案
    #[test]
    fn layouts_reference_existing_groups() {
        for id in HINT_LAYOUT {
            let binding = group(*id).expect("提示条引用了不存在的绑定组");
            assert!(binding.hint.is_some(), "{id:?} 进提示条但缺少 hint 文案");
        }
        for id in HELP_LEFT.iter().chain(HELP_RIGHT) {
            assert!(group(*id).is_some(), "帮助浮层引用了不存在的绑定组");
        }
        // 视觉布局守卫:提示条 11 项,帮助两栏各 8 项
        assert_eq!(hint_entries().count(), 11);
        let (left, right) = help_columns();
        assert_eq!((left.len(), right.len()), (8, 8));
    }

    /// 每个绑定组的标识动作必须能从自身键位触发(防布局引用悬空)
    #[test]
    fn group_ids_are_reachable_from_own_keys() {
        for binding in BINDINGS {
            assert!(
                binding.keys.iter().any(|(_, action)| *action == binding.id),
                "组 {:?} 的标识动作不在自身键位中",
                binding.id
            );
        }
    }
}
