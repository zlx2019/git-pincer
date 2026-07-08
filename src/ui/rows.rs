//! 渲染行的数据结构与构建:把块序列展开为三栏平行的行,含稳定块折叠与结果栏占位。

use crate::app::{FileMerge, SideState};
use crate::merge::{ChunkKind, MergeChunk};

/// 稳定块折叠阈值与首尾保留行数(与 Web 版一致)。
pub(crate) const FOLD_THRESHOLD: usize = 8;
pub(crate) const FOLD_KEEP: usize = 3;

/// 块的改动类型,决定色带颜色(IDEA 语义:蓝=修改、绿=新增、灰=删除、红=冲突)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeType {
    /// 双方均未改动
    None,
    /// 修改已有行
    Modified,
    /// 新增行(base 无此区间)
    Added,
    /// 删除行(改动侧为空)
    Deleted,
    /// 双方改动冲突
    Conflict,
}

/// 由块类型与两侧内容推断改动类型;agree 块按其改动性质归类(非冲突的普通改动)。
fn change_type(chunk: &MergeChunk) -> ChangeType {
    let of = |side: &[String]| {
        if chunk.base.is_empty() {
            ChangeType::Added
        } else if side.is_empty() {
            ChangeType::Deleted
        } else {
            ChangeType::Modified
        }
    };
    match chunk.kind {
        ChunkKind::Stable => ChangeType::None,
        ChunkKind::Conflict => ChangeType::Conflict,
        ChunkKind::Ours => of(&chunk.ours),
        ChunkKind::Theirs => of(&chunk.theirs),
        ChunkKind::Agree => of(&chunk.ours),
    }
}

/// 一栏中的一个单元。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell {
    /// 正常内容行
    Line {
        /// 该栏内的绝对行号(1-based)
        no: usize,
        /// 块内行偏移(词级强调寻址用)
        offset: usize,
        /// 行文本(不含换行)
        text: String,
    },
    /// 该栏在此行无内容(块内两侧行数不齐)
    Empty,
    /// 结果栏的「待解决」占位(未解决的冲突块)
    Placeholder,
}

/// 一个渲染行:三栏各自的单元,或跨三栏的折叠提示。
pub(crate) struct Row {
    /// 所属块下标(词级强调寻址用)
    pub(crate) chunk: usize,
    /// 块的改动类型(色带 / gutter 配色用)
    pub(crate) change: ChangeType,
    pub(crate) resolved: bool,
    pub(crate) current: bool,
    /// 折叠行:记录被折叠的行数,展示文案由渲染层生成
    pub(crate) fold: Option<usize>,
    /// 本地侧处理状态;None 表示该块本地侧无改动
    pub(crate) ours_state: Option<SideState>,
    /// 远端侧处理状态;None 表示该块远端侧无改动
    pub(crate) theirs_state: Option<SideState>,
    pub(crate) ours: Cell,
    pub(crate) result: Cell,
    pub(crate) theirs: Cell,
}

/// 把文件的块序列展开为渲染行,并返回每块首行的行下标(滚动定位用)。
pub(crate) fn build_rows(merge: &FileMerge, folded: bool) -> (Vec<Row>, Vec<usize>) {
    let mut rows: Vec<Row> = Vec::new();
    let mut chunk_starts: Vec<usize> = Vec::new();
    let mut result_no = 1usize;

    for (idx, chunk) in merge.chunks.iter().enumerate() {
        chunk_starts.push(rows.len());
        let resolved = merge.chunk_resolved(idx);
        let current = idx == merge.cursor && chunk.kind != ChunkKind::Stable;
        let change = change_type(chunk);
        let result_lines = merge.current_content(idx);
        // 各侧处理状态(gutter 符号用);单侧块只有对应一侧
        let st = &merge.states[idx];
        let (ours_state, theirs_state) = match chunk.kind {
            ChunkKind::Ours => (Some(st.ours), None),
            ChunkKind::Theirs => (None, Some(st.theirs)),
            ChunkKind::Agree | ChunkKind::Conflict => (Some(st.ours), Some(st.theirs)),
            ChunkKind::Stable => (None, None),
        };

        let height = chunk
            .ours
            .len()
            .max(result_lines.len())
            .max(chunk.theirs.len())
            .max(1);
        // 未解决的冲突块:结果栏不展示 base,改为居中一行「待解决」占位
        let placeholder_at = (chunk.kind == ChunkKind::Conflict && !resolved).then_some(height / 2);

        // 生成一段三栏平行行;i 为块内偏移
        let push_slice = |rows: &mut Vec<Row>, range: std::ops::Range<usize>| {
            for i in range {
                let cell = |lines: &[String], start: usize| match lines.get(i) {
                    Some(t) => Cell::Line {
                        no: start + i,
                        offset: i,
                        text: t.clone(),
                    },
                    None => Cell::Empty,
                };
                rows.push(Row {
                    chunk: idx,
                    change,
                    resolved,
                    current,
                    fold: None,
                    ours_state,
                    theirs_state,
                    ours: cell(&chunk.ours, chunk.ours_start),
                    result: match placeholder_at {
                        Some(at) if i == at => Cell::Placeholder,
                        Some(_) => Cell::Empty,
                        None => cell(&result_lines, result_no),
                    },
                    theirs: cell(&chunk.theirs, chunk.theirs_start),
                });
            }
        };

        let len = chunk.base.len();
        if chunk.kind == ChunkKind::Stable && folded && len > FOLD_THRESHOLD {
            push_slice(&mut rows, 0..FOLD_KEEP);
            rows.push(Row {
                chunk: idx,
                change,
                resolved,
                current: false,
                fold: Some(len - FOLD_KEEP * 2),
                ours_state,
                theirs_state,
                ours: Cell::Empty,
                result: Cell::Empty,
                theirs: Cell::Empty,
            });
            push_slice(&mut rows, len - FOLD_KEEP..len);
        } else {
            push_slice(&mut rows, 0..height);
        }
        result_no += result_lines.len();
    }
    (rows, chunk_starts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Side;

    /// 折叠行与行号构建正确
    #[test]
    fn build_rows_folds_long_stable_chunks() {
        let base: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let merge = FileMerge::from_three_way("demo.txt".to_owned(), &base, &base, &base);
        let (rows, starts) = build_rows(&merge, true);
        // 20 行稳定区折叠为 3 + 折叠行 + 3
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[3].fold, Some(14));
        assert_eq!(starts, vec![0]);

        let (unfolded, _) = build_rows(&merge, false);
        assert_eq!(unfolded.len(), 20);
    }

    /// 改动类型推断:新增 / 删除 / 修改 / 冲突(IDEA 四色语义)
    #[test]
    fn change_type_maps_idea_semantics() {
        let v = |s: &[&str]| s.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        let mk = |kind, base: &[&str], ours: &[&str], theirs: &[&str]| MergeChunk {
            id: 0,
            kind,
            base: v(base),
            ours: v(ours),
            theirs: v(theirs),
            base_start: 1,
            ours_start: 1,
            theirs_start: 1,
        };
        // 单侧新增(base 为空)
        let c = mk(ChunkKind::Ours, &[], &["new"], &[]);
        assert_eq!(change_type(&c), ChangeType::Added);
        // 单侧删除(改动侧为空)
        let c = mk(ChunkKind::Theirs, &["old"], &["old"], &[]);
        assert_eq!(change_type(&c), ChangeType::Deleted);
        // 单侧修改
        let c = mk(ChunkKind::Ours, &["a"], &["b"], &["a"]);
        assert_eq!(change_type(&c), ChangeType::Modified);
        // 冲突恒为冲突
        let c = mk(ChunkKind::Conflict, &[], &["x"], &["y"]);
        assert_eq!(change_type(&c), ChangeType::Conflict);
        // 双方一致按改动性质归类
        let c = mk(ChunkKind::Agree, &[], &["n"], &["n"]);
        assert_eq!(change_type(&c), ChangeType::Added);
    }

    /// 未解决冲突块的结果栏为占位;解决后恢复为内容行
    #[test]
    fn result_shows_placeholder_until_conflict_resolved() {
        let mut merge =
            FileMerge::from_three_way("demo.txt".to_owned(), "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        let (rows, _) = build_rows(&merge, false);
        assert!(rows.iter().any(|r| r.result == Cell::Placeholder));

        // 取本地、忽略远端 → 冲突解决,占位消失且结果为取用内容
        merge.apply(Side::Ours);
        merge.ignore(Side::Theirs);
        let (rows, _) = build_rows(&merge, false);
        assert!(rows.iter().all(|r| r.result != Cell::Placeholder));
        assert!(rows.iter().any(|r| matches!(
            &r.result,
            Cell::Line { text, .. } if text == "X"
        )));
    }
}
