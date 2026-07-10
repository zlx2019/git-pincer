//! State machine of a conflict-resolution session (pure logic; touches neither the terminal nor git).
//!
//! The model mirrors the frontend semantics of the toolkit-rs web version:
//! each side of a change chunk is in one of three states (pending / applied /
//! ignored), applied content is appended in take order (taking both sides of a
//! conflict one after another means "keep both"), and editing via `$EDITOR`
//! overrides the whole chunk.

use crate::merge::{ChunkKind, MergeChunk, MergeResult, merge_text};

/// 取用方向(改动来源侧)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// 本地(左栏)
    Ours,
    /// 远端(右栏)
    Theirs,
}

/// 一侧改动的处理状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideState {
    /// 待处理
    Pending,
    /// 已取用进结果
    Applied,
    /// 已忽略(不进入结果)
    Ignored,
}

/// 单个块的解决状态。
#[derive(Debug, Clone)]
pub struct ChunkState {
    /// 本地侧状态
    pub ours: SideState,
    /// 远端侧状态
    pub theirs: SideState,
    /// 取用顺序,决定内容拼接顺序
    pub order: Vec<Side>,
    /// $EDITOR 手动编辑后的整块覆写内容
    pub override_lines: Option<Vec<String>>,
}

impl ChunkState {
    /// 初始待处理状态。
    fn new() -> Self {
        Self {
            ours: SideState::Pending,
            theirs: SideState::Pending,
            order: Vec::new(),
            override_lines: None,
        }
    }

    /// 读取某一侧的状态。
    fn side(&self, side: Side) -> SideState {
        match side {
            Side::Ours => self.ours,
            Side::Theirs => self.theirs,
        }
    }

    /// 写入某一侧的状态。
    fn set_side(&mut self, side: Side, state: SideState) {
        match side {
            Side::Ours => self.ours = state,
            Side::Theirs => self.theirs = state,
        }
    }
}

/// 块上有待处理改动的侧:单侧块只有对应一侧,agree / conflict 两侧都有。
fn effective_sides(kind: ChunkKind) -> &'static [Side] {
    match kind {
        ChunkKind::Ours => &[Side::Ours],
        ChunkKind::Theirs => &[Side::Theirs],
        ChunkKind::Agree | ChunkKind::Conflict => &[Side::Ours, Side::Theirs],
        ChunkKind::Stable => &[],
    }
}

/// 一个文本文件的合并会话:块列表 + 各块解决状态 + 光标。
#[derive(Debug)]
pub struct FileMerge {
    /// 文件路径(git 模式为仓库根相对;单文件模式为输入路径)
    pub path: String,
    /// 合并块,首尾相接覆盖整个文件
    pub chunks: Vec<MergeChunk>,
    /// 与 chunks 一一对应的解决状态
    pub states: Vec<ChunkState>,
    /// 光标所在块下标(始终指向非 Stable 块;无改动块时为 0)
    pub cursor: usize,
    /// 视口滚动行偏移(由 UI 维护)
    pub scroll: usize,
    /// 视口是否跟随光标块(手动滚动后为 false,作用于光标的动作恢复跟随)
    pub follow: bool,
    /// 本地侧栏头标签(分支名)
    pub ours_label: Option<String>,
    /// 远端侧栏头标签(分支名)
    pub theirs_label: Option<String>,
    /// 任一原始输入以换行结尾时,导出内容补末尾换行
    ends_with_newline: bool,
}

impl FileMerge {
    /// 由三份完整文本构建(git 三方模式)。
    pub fn from_three_way(path: String, base: &str, ours: &str, theirs: &str) -> Self {
        let ends = base.ends_with('\n') || ours.ends_with('\n') || theirs.ends_with('\n');
        Self::from_result(path, merge_text(base, ours, theirs), ends)
    }

    /// 由已计算好的合并结果构建(冲突文件解析模式)。
    pub fn from_result(path: String, result: MergeResult, ends_with_newline: bool) -> Self {
        let states = result.chunks.iter().map(|_| ChunkState::new()).collect();
        let cursor = result
            .chunks
            .iter()
            .position(|c| c.kind != ChunkKind::Stable)
            .unwrap_or(0);
        Self {
            path,
            states,
            cursor,
            scroll: 0,
            follow: true,
            ours_label: result.ours_label,
            theirs_label: result.theirs_label,
            chunks: result.chunks,
            ends_with_newline,
        }
    }

    // —— 查询 ——

    /// 某块是否已处理完毕(被覆写,或所有有效侧均非待处理)。
    pub fn chunk_resolved(&self, idx: usize) -> bool {
        let state = &self.states[idx];
        if state.override_lines.is_some() {
            return true;
        }
        effective_sides(self.chunks[idx].kind)
            .iter()
            .all(|&side| state.side(side) != SideState::Pending)
    }

    /// 未处理完的冲突块数量(写盘的先决条件是归零)。
    pub fn pending_conflicts(&self) -> usize {
        (0..self.chunks.len())
            .filter(|&i| self.chunks[i].kind == ChunkKind::Conflict && !self.chunk_resolved(i))
            .count()
    }

    /// 未处理完的改动块总数(含非冲突,状态条展示用)。
    pub fn pending_changes(&self) -> usize {
        (0..self.chunks.len())
            .filter(|&i| self.chunks[i].kind != ChunkKind::Stable && !self.chunk_resolved(i))
            .count()
    }

    /// 文件是否可写盘:所有冲突块已解决(非冲突块保持待处理 = 保留 base)。
    pub fn ready_to_write(&self) -> bool {
        self.pending_conflicts() == 0
    }

    /// 某块当前在结果中的内容:覆写 > 按取用顺序拼接 > base。
    pub fn current_content(&self, idx: usize) -> Vec<String> {
        let chunk = &self.chunks[idx];
        let state = &self.states[idx];
        if let Some(lines) = &state.override_lines {
            return lines.clone();
        }
        if chunk.kind == ChunkKind::Stable {
            return chunk.base.clone();
        }
        if state.order.is_empty() {
            return chunk.base.clone();
        }
        state
            .order
            .iter()
            .flat_map(|side| match side {
                Side::Ours => chunk.ours_lines().to_vec(),
                Side::Theirs => chunk.theirs_lines().to_vec(),
            })
            .collect()
    }

    /// 导出整个文件的最终内容(按块拼接 + 末尾换行策略)。
    pub fn resolved_content(&self) -> String {
        let lines: Vec<String> = (0..self.chunks.len())
            .flat_map(|i| self.current_content(i))
            .collect();
        let text = lines.join("\n");
        if self.ends_with_newline && !text.is_empty() {
            format!("{text}\n")
        } else {
            text
        }
    }

    // —— 操作(作用于光标所在块)——

    /// 取用某一侧:内容按点击顺序追加(冲突两侧先后取用 =「两者都要」);
    /// agree 块两侧内容一致,取任一侧即整块完成。
    pub fn apply(&mut self, side: Side) {
        let idx = self.cursor;
        let kind = self.chunks[idx].kind;
        if !effective_sides(kind).contains(&side) {
            return;
        }
        let state = &mut self.states[idx];
        if state.override_lines.is_some() || state.side(side) != SideState::Pending {
            return;
        }
        if kind == ChunkKind::Agree {
            state.ours = SideState::Applied;
            state.theirs = SideState::Applied;
            state.order = vec![Side::Ours];
        } else {
            state.set_side(side, SideState::Applied);
            state.order.push(side);
        }
    }

    /// 忽略某一侧(该侧改动不进入结果);agree 块忽略即整块保留 base。
    pub fn ignore(&mut self, side: Side) {
        let idx = self.cursor;
        let kind = self.chunks[idx].kind;
        if !effective_sides(kind).contains(&side) {
            return;
        }
        let state = &mut self.states[idx];
        if state.override_lines.is_some() || state.side(side) != SideState::Pending {
            return;
        }
        if kind == ChunkKind::Agree {
            state.ours = SideState::Ignored;
            state.theirs = SideState::Ignored;
        } else {
            state.set_side(side, SideState::Ignored);
        }
    }

    /// 撤销当前块的全部决定,回到待处理。
    pub fn undo(&mut self) {
        self.states[self.cursor] = ChunkState::new();
    }

    /// 撤销本文件所有块的决定(含 $EDITOR 覆写),全部回到待处理。
    pub fn undo_all(&mut self) {
        for state in &mut self.states {
            *state = ChunkState::new();
        }
    }

    /// 用 $EDITOR 编辑后的内容覆写当前块(视为已解决)。
    pub fn set_override(&mut self, lines: Vec<String>) {
        self.states[self.cursor].override_lines = Some(lines);
    }

    /// 一键取用所有非冲突改动(ours/theirs/agree)。
    pub fn apply_all_nonconflict(&mut self) {
        let saved = self.cursor;
        for idx in 0..self.chunks.len() {
            let kind = self.chunks[idx].kind;
            if matches!(kind, ChunkKind::Stable | ChunkKind::Conflict) || self.chunk_resolved(idx) {
                continue;
            }
            self.cursor = idx;
            // 有效侧的第一个即该块的改动来源(agree 取任一侧等价)
            if let Some(&side) = effective_sides(kind).first() {
                self.apply(side);
            }
        }
        self.cursor = saved;
    }

    // —— 光标移动 ——

    /// 移到下一个改动块(跳过 Stable)。
    pub fn next_change(&mut self) {
        if let Some(idx) =
            (self.cursor + 1..self.chunks.len()).find(|&i| self.chunks[i].kind != ChunkKind::Stable)
        {
            self.cursor = idx;
        }
    }

    /// 移到上一个改动块(跳过 Stable)。
    pub fn prev_change(&mut self) {
        if let Some(idx) = (0..self.cursor)
            .rev()
            .find(|&i| self.chunks[i].kind != ChunkKind::Stable)
        {
            self.cursor = idx;
        }
    }

    /// 跳到下一个未解决的冲突块(到底后从头环绕)。
    pub fn next_conflict(&mut self) {
        let n = self.chunks.len();
        if let Some(idx) = (1..=n)
            .map(|step| (self.cursor + step) % n)
            .find(|&i| self.chunks[i].kind == ChunkKind::Conflict && !self.chunk_resolved(i))
        {
            self.cursor = idx;
        }
    }

    /// 跳到上一个未解决的冲突块(到头后从尾环绕)。
    pub fn prev_conflict(&mut self) {
        let n = self.chunks.len();
        if let Some(idx) = (1..=n)
            .map(|step| (self.cursor + n - step) % n)
            .find(|&i| self.chunks[i].kind == ChunkKind::Conflict && !self.chunk_resolved(i))
        {
            self.cursor = idx;
        }
    }
}

/// 会话中的单个待解决文件。
#[derive(Debug)]
pub enum FileEntry {
    /// 文本文件:三栏逐块解决
    Text(FileMerge),
    /// 二进制文件:降级为整文件二选一
    Binary {
        /// 文件路径
        path: String,
        /// 本地侧完整字节
        ours: Vec<u8>,
        /// 远端侧完整字节
        theirs: Vec<u8>,
        /// 用户的选择
        choice: Option<Side>,
    },
}

impl FileEntry {
    /// 文件路径。
    pub fn path(&self) -> &str {
        match self {
            FileEntry::Text(m) => &m.path,
            FileEntry::Binary { path, .. } => path,
        }
    }

    /// 是否满足写盘条件。
    pub fn ready_to_write(&self) -> bool {
        match self {
            FileEntry::Text(m) => m.ready_to_write(),
            FileEntry::Binary { choice, .. } => choice.is_some(),
        }
    }

    /// 导出最终字节内容(前提:ready_to_write)。
    pub fn resolved_bytes(&self) -> Vec<u8> {
        match self {
            FileEntry::Text(m) => m.resolved_content().into_bytes(),
            FileEntry::Binary {
                ours,
                theirs,
                choice,
                ..
            } => match choice {
                Some(Side::Theirs) => theirs.clone(),
                _ => ours.clone(),
            },
        }
    }
}

/// 多文件冲突解决会话。
#[derive(Debug)]
pub struct Session {
    /// 全部待解决文件
    pub files: Vec<FileEntry>,
    /// 当前文件下标
    pub current: usize,
    /// 各文件是否已写盘
    pub written: Vec<bool>,
    /// 状态条展示的操作名(如 "merge" / "rebase" / 文件名)
    pub op_label: String,
    /// 是否折叠长稳定区(z 切换)
    pub folded: bool,
}

impl Session {
    /// 构建会话,光标落在第一个文件。
    pub fn new(files: Vec<FileEntry>, op_label: String) -> Self {
        let written = files.iter().map(|_| false).collect();
        Self {
            files,
            current: 0,
            written,
            op_label,
            folded: true,
        }
    }

    /// 当前文件。
    pub fn current_file(&self) -> &FileEntry {
        &self.files[self.current]
    }

    /// 当前文件(可变)。
    pub fn current_file_mut(&mut self) -> &mut FileEntry {
        &mut self.files[self.current]
    }

    /// 标记当前文件已写盘,并把光标移到下一个未写盘文件(若有)。
    pub fn mark_written(&mut self) {
        self.written[self.current] = true;
        if let Some(idx) = (0..self.files.len()).find(|&i| !self.written[i]) {
            self.current = idx;
        }
    }

    /// 是否所有文件都已写盘。
    pub fn all_written(&self) -> bool {
        self.written.iter().all(|&w| w)
    }

    /// 切换到下一个文件(环绕)。
    pub fn next_file(&mut self) {
        if !self.files.is_empty() {
            self.current = (self.current + 1) % self.files.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个含 [Stable, Conflict, Stable, Ours] 的会话文件
    fn sample() -> FileMerge {
        FileMerge::from_three_way(
            "demo.txt".to_owned(),
            "a\nb\nc\nd\n",
            "a\nX\nc\nD\n",
            "a\nY\nc\nd\n",
        )
    }

    #[test]
    fn sample_shape_is_expected() {
        let merge = sample();
        let kinds: Vec<ChunkKind> = merge.chunks.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ChunkKind::Stable,
                ChunkKind::Conflict,
                ChunkKind::Stable,
                ChunkKind::Ours
            ]
        );
        // 光标初始落在第一个改动块
        assert_eq!(merge.cursor, 1);
    }

    #[test]
    fn apply_ours_resolves_conflict_side() {
        let mut merge = sample();
        merge.apply(Side::Ours);
        assert!(!merge.chunk_resolved(1)); // theirs 侧仍待处理
        merge.ignore(Side::Theirs);
        assert!(merge.chunk_resolved(1));
        assert_eq!(merge.current_content(1), vec!["X"]);
    }

    #[test]
    fn apply_both_sides_appends_in_order() {
        let mut merge = sample();
        merge.apply(Side::Theirs);
        merge.apply(Side::Ours);
        assert_eq!(merge.current_content(1), vec!["Y", "X"]);
        assert!(merge.chunk_resolved(1));
    }

    #[test]
    fn ignore_both_keeps_base() {
        let mut merge = sample();
        merge.ignore(Side::Ours);
        merge.ignore(Side::Theirs);
        assert_eq!(merge.current_content(1), vec!["b"]);
        assert!(merge.chunk_resolved(1));
    }

    #[test]
    fn undo_restores_pending() {
        let mut merge = sample();
        merge.apply(Side::Ours);
        merge.undo();
        assert!(!merge.chunk_resolved(1));
        assert_eq!(merge.current_content(1), vec!["b"]);
    }

    #[test]
    fn undo_all_resets_every_chunk() {
        let mut merge = sample();
        merge.apply(Side::Ours);
        merge.ignore(Side::Theirs);
        merge.cursor = 3;
        merge.apply(Side::Ours);
        merge.set_override(vec!["edited".to_owned()]);
        assert_eq!(merge.pending_changes(), 0);
        merge.undo_all();
        assert_eq!(merge.pending_changes(), 2);
        assert!(!merge.chunk_resolved(1));
        assert!(!merge.chunk_resolved(3));
        assert_eq!(merge.current_content(3), vec!["d"]);
    }

    #[test]
    fn override_wins_and_resolves() {
        let mut merge = sample();
        merge.set_override(vec!["merged".to_owned()]);
        assert!(merge.chunk_resolved(1));
        assert_eq!(merge.current_content(1), vec!["merged"]);
        // 覆写后取用不再生效
        merge.apply(Side::Ours);
        assert_eq!(merge.current_content(1), vec!["merged"]);
    }

    #[test]
    fn apply_on_ineffective_side_is_noop() {
        let mut merge = sample();
        merge.cursor = 3; // Ours 块
        merge.apply(Side::Theirs);
        assert!(!merge.chunk_resolved(3));
        merge.apply(Side::Ours);
        assert!(merge.chunk_resolved(3));
        assert_eq!(merge.current_content(3), vec!["D"]);
    }

    #[test]
    fn apply_all_nonconflict_skips_conflicts() {
        let mut merge = sample();
        merge.apply_all_nonconflict();
        assert!(merge.chunk_resolved(3));
        assert!(!merge.chunk_resolved(1));
        assert_eq!(merge.pending_conflicts(), 1);
    }

    #[test]
    fn resolved_content_joins_chunks_with_newline() {
        let mut merge = sample();
        merge.apply(Side::Ours);
        merge.ignore(Side::Theirs);
        merge.cursor = 3;
        merge.apply(Side::Ours);
        assert!(merge.ready_to_write());
        assert_eq!(merge.resolved_content(), "a\nX\nc\nD\n");
    }

    #[test]
    fn conflict_navigation_wraps_and_skips_resolved() {
        let mut merge = FileMerge::from_three_way(
            "demo.txt".to_owned(),
            "a\nb\nc\nd\ne\n",
            "a\nX\nc\nY\ne\n",
            "a\nP\nc\nQ\ne\n",
        );
        // 两个冲突:块 1 与块 3
        assert_eq!(merge.pending_conflicts(), 2);
        assert_eq!(merge.cursor, 1);
        merge.next_conflict();
        assert_eq!(merge.cursor, 3);
        merge.next_conflict();
        assert_eq!(merge.cursor, 1); // 环绕
        // 解决块 1 后,导航只在剩余冲突上
        merge.apply(Side::Ours);
        merge.ignore(Side::Theirs);
        merge.next_conflict();
        assert_eq!(merge.cursor, 3);
        merge.next_conflict();
        assert_eq!(merge.cursor, 3);
    }

    #[test]
    fn agree_single_apply_resolves_both_sides() {
        let mut merge =
            FileMerge::from_three_way("demo.txt".to_owned(), "a\nb\n", "a\nB\n", "a\nB\n");
        assert_eq!(merge.chunks[merge.cursor].kind, ChunkKind::Agree);
        merge.apply(Side::Theirs);
        assert!(merge.chunk_resolved(merge.cursor));
        assert_eq!(merge.current_content(merge.cursor), vec!["B"]);
    }

    #[test]
    fn session_marks_written_and_advances() {
        let files = vec![
            FileEntry::Text(sample()),
            FileEntry::Binary {
                path: "logo.png".to_owned(),
                ours: vec![1, 2],
                theirs: vec![3, 4],
                choice: None,
            },
        ];
        let mut session = Session::new(files, "merge".to_owned());
        assert!(!session.all_written());
        session.mark_written();
        assert_eq!(session.current, 1);
        // 二进制文件选边后可写
        if let FileEntry::Binary { choice, .. } = session.current_file_mut() {
            *choice = Some(Side::Theirs);
        }
        assert!(session.current_file().ready_to_write());
        assert_eq!(session.current_file().resolved_bytes(), vec![3, 4]);
        session.mark_written();
        assert!(session.all_written());
    }
}
