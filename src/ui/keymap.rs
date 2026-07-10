//! Central key-binding table: dispatch, the hint bar and the help overlay
//! all derive from the single [`BINDINGS`] source, so a key can never drift
//! out of sync between behavior and documentation.

use ratatui::crossterm::event::KeyCode;

/// 冲突解决会话内的可绑定动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// 一组按键绑定:展示上是一行(如 `u / U`),分发上可含多个键位。
pub(crate) struct Binding {
    /// 组的标识动作(布局列表以此引用)
    id: Action,
    /// 键位 → 动作(如 u 撤销块、U 撤销整个文件同属一组)
    keys: &'static [(KeyCode, Action)],
    /// 帮助浮层中的按键标签
    label: &'static str,
    /// 提示条中的紧凑按键标签
    hint_label: &'static str,
    /// 提示条短文案的 i18n key;None 表示该组不进提示条
    hint: Option<&'static str>,
    /// 帮助浮层文案的 i18n key
    help: &'static str,
}

/// 全部按键绑定(单一事实来源)。
static BINDINGS: &[Binding] = &[
    Binding {
        id: Action::TakeLocal,
        keys: &[
            (KeyCode::Char('h'), Action::TakeLocal),
            (KeyCode::Left, Action::TakeLocal),
        ],
        label: "h / ←",
        hint_label: "h",
        hint: Some("ui.hint_left"),
        help: "ui.help_left",
    },
    Binding {
        id: Action::TakeRemote,
        keys: &[
            (KeyCode::Char('l'), Action::TakeRemote),
            (KeyCode::Right, Action::TakeRemote),
        ],
        label: "l / →",
        hint_label: "l",
        hint: Some("ui.hint_right"),
        help: "ui.help_right",
    },
    Binding {
        id: Action::IgnoreChunk,
        keys: &[(KeyCode::Char('x'), Action::IgnoreChunk)],
        label: "x",
        hint_label: "x",
        hint: Some("ui.hint_ignore"),
        help: "ui.help_ignore",
    },
    Binding {
        id: Action::UndoChunk,
        keys: &[
            (KeyCode::Char('u'), Action::UndoChunk),
            (KeyCode::Char('U'), Action::UndoFile),
        ],
        label: "u / U",
        hint_label: "u/U",
        hint: Some("ui.hint_undo"),
        help: "ui.help_undo",
    },
    Binding {
        id: Action::EditChunk,
        keys: &[(KeyCode::Char('e'), Action::EditChunk)],
        label: "e",
        hint_label: "e",
        hint: Some("ui.hint_edit"),
        help: "ui.help_edit",
    },
    Binding {
        id: Action::ApplyNonConflict,
        keys: &[(KeyCode::Char('a'), Action::ApplyNonConflict)],
        label: "a",
        hint_label: "a",
        hint: None,
        help: "ui.help_apply",
    },
    Binding {
        id: Action::WriteFile,
        keys: &[(KeyCode::Char('w'), Action::WriteFile)],
        label: "w",
        hint_label: "w",
        hint: Some("ui.hint_write"),
        help: "ui.help_write",
    },
    Binding {
        id: Action::Quit,
        keys: &[(KeyCode::Char('q'), Action::Quit)],
        label: "q",
        hint_label: "q",
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
        label: "j / k",
        hint_label: "j/k",
        hint: None,
        help: "ui.help_move",
    },
    Binding {
        id: Action::NextConflict,
        keys: &[
            (KeyCode::Char('n'), Action::NextConflict),
            (KeyCode::Char('p'), Action::PrevConflict),
        ],
        label: "n / p",
        hint_label: "n/p",
        hint: Some("ui.hint_conflict"),
        help: "ui.help_jump",
    },
    Binding {
        id: Action::NextFile,
        keys: &[(KeyCode::Tab, Action::NextFile)],
        label: "Tab",
        hint_label: "⇥",
        hint: Some("ui.hint_file"),
        help: "ui.help_tab",
    },
    Binding {
        id: Action::ToggleFold,
        keys: &[(KeyCode::Char('z'), Action::ToggleFold)],
        label: "z",
        hint_label: "z",
        hint: Some("ui.hint_fold"),
        help: "ui.help_fold",
    },
    Binding {
        id: Action::CopyChunk,
        keys: &[(KeyCode::Char('y'), Action::CopyChunk)],
        label: "y",
        hint_label: "y",
        hint: None,
        help: "ui.help_copy_chunk",
    },
    Binding {
        id: Action::CopyFile,
        keys: &[(KeyCode::Char('Y'), Action::CopyFile)],
        label: "Y",
        hint_label: "Y",
        hint: None,
        help: "ui.help_copy_file",
    },
    Binding {
        id: Action::CopyLocal,
        keys: &[
            (KeyCode::Char('H'), Action::CopyLocal),
            (KeyCode::Char('L'), Action::CopyRemote),
        ],
        label: "H / L",
        hint_label: "H/L",
        hint: None,
        help: "ui.help_copy_sides",
    },
    Binding {
        id: Action::Help,
        keys: &[(KeyCode::Char('?'), Action::Help)],
        label: "?",
        hint_label: "?",
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

/// 把按键解析为动作;未绑定的键返回 None。
pub(crate) fn action_for(code: KeyCode) -> Option<Action> {
    BINDINGS
        .iter()
        .flat_map(|b| b.keys)
        .find(|(key, _)| *key == code)
        .map(|(_, action)| *action)
}

/// 按标识动作取绑定组(布局列表引用用)。
fn group(id: Action) -> Option<&'static Binding> {
    BINDINGS.iter().find(|b| b.id == id)
}

/// 提示条条目:(紧凑按键标签, 文案 i18n key),按展示顺序。
pub(crate) fn hint_entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    HINT_LAYOUT.iter().filter_map(|id| {
        let binding = group(*id)?;
        Some((binding.hint_label, binding.hint?))
    })
}

/// 帮助条目:(按键标签, 文案 i18n key)。
pub(crate) type HelpEntry = (&'static str, &'static str);

/// 帮助浮层某一栏的条目。
fn help_entries(layout: &'static [Action]) -> Vec<HelpEntry> {
    layout
        .iter()
        .filter_map(|id| group(*id).map(|b| (b.label, b.help)))
        .collect()
}

/// 帮助浮层的左 / 右两栏条目。
pub(crate) fn help_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    (help_entries(HELP_LEFT), help_entries(HELP_RIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 常用键解析到预期动作;未绑定键返回 None
    #[test]
    fn keys_resolve_to_expected_actions() {
        assert_eq!(action_for(KeyCode::Char('h')), Some(Action::TakeLocal));
        assert_eq!(action_for(KeyCode::Left), Some(Action::TakeLocal));
        assert_eq!(action_for(KeyCode::Char('U')), Some(Action::UndoFile));
        assert_eq!(action_for(KeyCode::Char('k')), Some(Action::PrevChange));
        assert_eq!(action_for(KeyCode::Tab), Some(Action::NextFile));
        assert_eq!(action_for(KeyCode::Char('?')), Some(Action::Help));
        assert_eq!(action_for(KeyCode::Char('0')), None);
        assert_eq!(action_for(KeyCode::Esc), None);
    }

    /// 键位在全表内无重复绑定(新增按键时的冲突守卫)
    #[test]
    fn no_duplicate_key_bindings() {
        let mut seen = BTreeSet::new();
        for (key, _) in BINDINGS.iter().flat_map(|b| b.keys) {
            assert!(seen.insert(format!("{key:?}")), "键位 {key:?} 被绑定了多次");
        }
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
