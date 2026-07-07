//! Three-way merge core: diff3-style chunking built on top of two 2-way diffs from `similar`.
//!
//! Ported from the module of the same name in toolkit-rs (with the web
//! serialization parts removed). The algorithm: compute line-level diffs for
//! base→ours and base→theirs, extract every non-Equal op as a hunk (a base
//! line range plus its replacement lines), then group hunks from both sides by
//! collisions between their base ranges. A group touched by only one side
//! applies cleanly; identical changes from both sides count as agreement;
//! everything else is marked as a conflict and handed to the interactive TUI.

use std::ops::Range;
use std::time::Duration;

use similar::{Algorithm, DiffTag, TextDiff};

/// 单次 diff 的最长计算时间,超时后 similar 会降级为较粗粒度的结果,
/// 保证超大文件下 TUI 仍能及时进入交互。
const DIFF_TIMEOUT: Duration = Duration::from_millis(500);

/// 合并块的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// 双方均未改动,内容与 base 一致
    Stable,
    /// 仅本地(ours)改动,可无冲突应用
    Ours,
    /// 仅远端(theirs)改动,可无冲突应用
    Theirs,
    /// 双方改动且内容一致,可无冲突应用
    Agree,
    /// 双方改动且内容不同,需人工解决
    Conflict,
}

/// 一个合并块:base 的某个连续区间以及双方在该区间的内容。
#[derive(Debug, Clone)]
pub struct MergeChunk {
    /// 块序号,供交互层定位操作
    pub id: usize,
    /// 块类型
    pub kind: ChunkKind,
    /// base 在该区间的行内容(已去行尾换行)
    pub base: Vec<String>,
    /// 本地侧在该区间的行内容(未改动时与 base 相同)
    pub ours: Vec<String>,
    /// 远端侧在该区间的行内容(未改动时与 base 相同)
    pub theirs: Vec<String>,
    /// base 侧起始行号(1-based)
    pub base_start: usize,
    /// 本地侧起始行号(1-based)
    pub ours_start: usize,
    /// 远端侧起始行号(1-based)
    pub theirs_start: usize,
}

/// 三方合并的完整结果:按文件顺序排列的块 + 统计信息。
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// 全部合并块,首尾相接覆盖整个文件
    pub chunks: Vec<MergeChunk>,
    /// 冲突块数量
    pub conflicts: usize,
    /// 仅本地改动的块数量
    pub ours_changes: usize,
    /// 仅远端改动的块数量
    pub theirs_changes: usize,
    /// 双方一致改动的块数量
    pub agree: usize,
    /// 本地侧标签(来自 `<<<<<<<` 后的分支名;三方模式下为 None)
    pub ours_label: Option<String>,
    /// 远端侧标签(来自 `>>>>>>>` 后的分支名;三方模式下为 None)
    pub theirs_label: Option<String>,
}

/// 单侧 hunk:base 的某个行区间被替换为新的行内容。
#[derive(Debug, Clone)]
struct Hunk {
    /// 被替换的 base 行区间(左闭右开;纯插入时为空区间)
    base_range: Range<usize>,
    /// 替换后的行内容(已去行尾换行)
    lines: Vec<String>,
}

/// hunk 的来源侧。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Ours,
    Theirs,
}

/// 带侧别标记的 hunk,分组时两侧混排使用。
#[derive(Debug, Clone)]
struct SidedHunk {
    side: Side,
    hunk: Hunk,
}

/// 对三份文本执行三方合并,返回按文件顺序排列的合并块与统计信息。
pub fn merge_text(base: &str, ours: &str, theirs: &str) -> MergeResult {
    let ours_diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DIFF_TIMEOUT)
        .diff_lines(base, ours);
    let theirs_diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DIFF_TIMEOUT)
        .diff_lines(base, theirs);

    // base 行以 similar 的分词为准,保证与 op 的区间下标一致
    let base_lines: Vec<String> = (0..ours_diff.old_len())
        .filter_map(|i| ours_diff.old_slice(i))
        .map(clean_line)
        .collect();

    let groups = group_hunks(extract_hunks(&ours_diff), extract_hunks(&theirs_diff));
    build_chunks(&base_lines, groups)
}

/// 去除行尾换行符(\n 或 \r\n)。
fn clean_line(line: &str) -> String {
    line.trim_end_matches(['\r', '\n']).to_owned()
}

/// 从一次 2-way diff 中提取全部非 Equal 操作为 hunk。
fn extract_hunks(diff: &TextDiff<'_, '_, str>) -> Vec<Hunk> {
    diff.ops()
        .iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| Hunk {
            base_range: op.old_range(),
            // 合法区间内 new_slice 必然有值,filter_map 仅为规避 unwrap
            lines: op
                .new_range()
                .filter_map(|i| diff.new_slice(i))
                .map(clean_line)
                .collect(),
        })
        .collect()
}

/// 判断两个 base 区间是否「碰撞」(需归入同一组处理)。
///
/// 保守策略:空区间(纯插入)与另一区间端点相接也算碰撞,宁可多报冲突,
/// 也不冒险静默合并出错误结果;两个非空区间仅端点相接则不算碰撞,
/// 双方改动可独立应用。
fn collides(a: &Range<usize>, b: &Range<usize>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => a.start == b.start,
        (true, false) => b.start <= a.start && a.start <= b.end,
        (false, true) => a.start <= b.start && b.start <= a.end,
        (false, false) => a.start.max(b.start) < a.end.min(b.end),
    }
}

/// 将两侧 hunk 按 base 区间的碰撞关系归并成组,组内保持 base 起点有序。
fn group_hunks(ours: Vec<Hunk>, theirs: Vec<Hunk>) -> Vec<Vec<SidedHunk>> {
    let mut all: Vec<SidedHunk> = ours
        .into_iter()
        .map(|hunk| SidedHunk {
            side: Side::Ours,
            hunk,
        })
        .chain(theirs.into_iter().map(|hunk| SidedHunk {
            side: Side::Theirs,
            hunk,
        }))
        .collect();
    // 按 base 起点排序;同起点时 ours 在前,保证结果确定性
    all.sort_by_key(|s| {
        (
            s.hunk.base_range.start,
            s.hunk.base_range.end,
            s.side == Side::Theirs,
        )
    });

    // 已排序前提下,只需用组的合并区间与下一个 hunk 判碰撞即可传递扩张
    let mut groups: Vec<(Range<usize>, Vec<SidedHunk>)> = Vec::new();
    for sided in all {
        let range = sided.hunk.base_range.clone();
        match groups.last_mut() {
            Some((merged, members)) if collides(merged, &range) => {
                merged.start = merged.start.min(range.start);
                merged.end = merged.end.max(range.end);
                members.push(sided);
            }
            _ => groups.push((range, vec![sided])),
        }
    }
    groups.into_iter().map(|(_, members)| members).collect()
}

/// 计算某一侧在组合 base 区间 [lo, hi) 内的最终内容:
/// 未被该侧 hunk 覆盖的 base 行保持原样,被覆盖处使用替换行。
fn side_content(
    base_lines: &[String],
    lo: usize,
    hi: usize,
    group: &[SidedHunk],
    side: Side,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = lo;
    for sided in group.iter().filter(|s| s.side == side) {
        out.extend_from_slice(&base_lines[pos..sided.hunk.base_range.start]);
        out.extend(sided.hunk.lines.iter().cloned());
        pos = sided.hunk.base_range.end;
    }
    out.extend_from_slice(&base_lines[pos..hi]);
    out
}

/// 由 hunk 组构建首尾相接的合并块序列,并统计各类块数量。
fn build_chunks(base_lines: &[String], groups: Vec<Vec<SidedHunk>>) -> MergeResult {
    let mut chunks: Vec<MergeChunk> = Vec::new();
    let mut conflicts = 0usize;
    let mut ours_changes = 0usize;
    let mut theirs_changes = 0usize;
    let mut agree = 0usize;

    // 三侧各自的行号游标(1-based)与 base 消费位置
    let mut base_pos = 0usize;
    let mut base_no = 1usize;
    let mut ours_no = 1usize;
    let mut theirs_no = 1usize;

    // 追加一个稳定块(双方内容与 base 一致)
    let push_stable = |chunks: &mut Vec<MergeChunk>,
                       lines: &[String],
                       base_no: &mut usize,
                       ours_no: &mut usize,
                       theirs_no: &mut usize| {
        chunks.push(MergeChunk {
            id: chunks.len(),
            kind: ChunkKind::Stable,
            base: lines.to_vec(),
            ours: lines.to_vec(),
            theirs: lines.to_vec(),
            base_start: *base_no,
            ours_start: *ours_no,
            theirs_start: *theirs_no,
        });
        *base_no += lines.len();
        *ours_no += lines.len();
        *theirs_no += lines.len();
    };

    for group in groups {
        // 组的组合 base 区间(组必非空,unwrap_or 仅为规避 unwrap)
        let lo = group
            .iter()
            .map(|s| s.hunk.base_range.start)
            .min()
            .unwrap_or(0);
        let hi = group
            .iter()
            .map(|s| s.hunk.base_range.end)
            .max()
            .unwrap_or(lo);

        // 组之前的稳定区
        if lo > base_pos {
            push_stable(
                &mut chunks,
                &base_lines[base_pos..lo],
                &mut base_no,
                &mut ours_no,
                &mut theirs_no,
            );
        }

        let ours_lines = side_content(base_lines, lo, hi, &group, Side::Ours);
        let theirs_lines = side_content(base_lines, lo, hi, &group, Side::Theirs);
        let has_ours = group.iter().any(|s| s.side == Side::Ours);
        let has_theirs = group.iter().any(|s| s.side == Side::Theirs);
        let kind = match (has_ours, has_theirs) {
            (true, false) => ChunkKind::Ours,
            (false, true) => ChunkKind::Theirs,
            _ if ours_lines == theirs_lines => ChunkKind::Agree,
            _ => ChunkKind::Conflict,
        };
        match kind {
            ChunkKind::Ours => ours_changes += 1,
            ChunkKind::Theirs => theirs_changes += 1,
            ChunkKind::Agree => agree += 1,
            ChunkKind::Conflict => conflicts += 1,
            ChunkKind::Stable => {}
        }

        chunks.push(MergeChunk {
            id: chunks.len(),
            kind,
            base: base_lines[lo..hi].to_vec(),
            ours: ours_lines.clone(),
            theirs: theirs_lines.clone(),
            base_start: base_no,
            ours_start: ours_no,
            theirs_start: theirs_no,
        });
        base_no += hi - lo;
        ours_no += ours_lines.len();
        theirs_no += theirs_lines.len();
        base_pos = hi;
    }

    // 末尾的稳定区
    if base_pos < base_lines.len() {
        push_stable(
            &mut chunks,
            &base_lines[base_pos..],
            &mut base_no,
            &mut ours_no,
            &mut theirs_no,
        );
    }

    MergeResult {
        chunks,
        conflicts,
        ours_changes,
        theirs_changes,
        agree,
        ours_label: None,
        theirs_label: None,
    }
}

/// 冲突文件解析错误。
#[derive(Debug, thiserror::Error)]
pub enum ConflictParseError {
    /// 整个文件没有任何冲突标记
    #[error("未检测到 git 冲突标记(<<<<<<< / ======= / >>>>>>>)")]
    NoMarkers,
    /// 冲突标记不完整或顺序错误
    #[error("第 {0} 行附近的冲突标记不完整或顺序错误")]
    Malformed(usize),
}

/// 解析时当前正在收集的冲突段落。
enum Section {
    /// 冲突块之外的公共内容
    Common,
    /// `<<<<<<<` 之后的本地内容
    Ours,
    /// `|||||||` 之后的 base 内容(diff3 风格才有)
    Base,
    /// `=======` 之后的远端内容
    Theirs,
}

/// 解析带 git 冲突标记的单个文件,还原为合并块序列。
///
/// git 合并时非冲突改动已写入文件,因此结果只含 Stable 与 Conflict 两类块;
/// 支持默认(merge)与 diff3 两种 conflictStyle,并提取 `<<<<<<< / >>>>>>>`
/// 后的分支标签。默认风格没有 base 段,冲突块的 base 为空。
pub fn parse_conflict_file(text: &str) -> Result<MergeResult, ConflictParseError> {
    let mut chunks: Vec<MergeChunk> = Vec::new();
    let mut conflicts = 0usize;
    let mut ours_label: Option<String> = None;
    let mut theirs_label: Option<String> = None;

    // 三侧各自的行号游标(1-based),指向重建后的各侧文档位置
    let mut base_no = 1usize;
    let mut ours_no = 1usize;
    let mut theirs_no = 1usize;

    // 各段落的行缓冲
    let mut common: Vec<String> = Vec::new();
    let mut ours: Vec<String> = Vec::new();
    let mut base: Vec<String> = Vec::new();
    let mut theirs: Vec<String> = Vec::new();
    let mut section = Section::Common;
    let mut conflict_start = 0usize;

    // 把缓冲的公共段落收为一个稳定块
    let flush_common = |chunks: &mut Vec<MergeChunk>,
                        common: &mut Vec<String>,
                        base_no: &mut usize,
                        ours_no: &mut usize,
                        theirs_no: &mut usize| {
        if common.is_empty() {
            return;
        }
        let lines = std::mem::take(common);
        chunks.push(MergeChunk {
            id: chunks.len(),
            kind: ChunkKind::Stable,
            base: lines.clone(),
            ours: lines.clone(),
            theirs: lines.clone(),
            base_start: *base_no,
            ours_start: *ours_no,
            theirs_start: *theirs_no,
        });
        *base_no += lines.len();
        *ours_no += lines.len();
        *theirs_no += lines.len();
    };

    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        let line_no = idx + 1;

        if let Some(rest) = line.strip_prefix("<<<<<<<") {
            // 冲突开始;嵌套的 <<<<<<< 视为格式错误
            if !matches!(section, Section::Common) {
                return Err(ConflictParseError::Malformed(line_no));
            }
            flush_common(
                &mut chunks,
                &mut common,
                &mut base_no,
                &mut ours_no,
                &mut theirs_no,
            );
            conflict_start = line_no;
            let label = rest.trim();
            if ours_label.is_none() && !label.is_empty() {
                ours_label = Some(label.to_owned());
            }
            section = Section::Ours;
        } else if matches!(section, Section::Ours) && line.starts_with("|||||||") {
            section = Section::Base;
        } else if matches!(section, Section::Ours | Section::Base)
            && line.len() >= 7
            && line.bytes().all(|b| b == b'=')
        {
            section = Section::Theirs;
        } else if let Some(rest) = line.strip_prefix(">>>>>>>") {
            if !matches!(section, Section::Theirs) {
                return Err(ConflictParseError::Malformed(line_no));
            }
            let label = rest.trim();
            if theirs_label.is_none() && !label.is_empty() {
                theirs_label = Some(label.to_owned());
            }
            let (base_lines, ours_lines, theirs_lines) = (
                std::mem::take(&mut base),
                std::mem::take(&mut ours),
                std::mem::take(&mut theirs),
            );
            chunks.push(MergeChunk {
                id: chunks.len(),
                kind: ChunkKind::Conflict,
                base_start: base_no,
                ours_start: ours_no,
                theirs_start: theirs_no,
                base: base_lines.clone(),
                ours: ours_lines.clone(),
                theirs: theirs_lines.clone(),
            });
            base_no += base_lines.len();
            ours_no += ours_lines.len();
            theirs_no += theirs_lines.len();
            conflicts += 1;
            section = Section::Common;
        } else {
            let owned = line.to_owned();
            match section {
                Section::Common => common.push(owned),
                Section::Ours => ours.push(owned),
                Section::Base => base.push(owned),
                Section::Theirs => theirs.push(owned),
            }
        }
    }

    // 文件结束时仍在冲突块内 → 标记不完整
    if !matches!(section, Section::Common) {
        return Err(ConflictParseError::Malformed(conflict_start));
    }
    if conflicts == 0 {
        return Err(ConflictParseError::NoMarkers);
    }
    flush_common(
        &mut chunks,
        &mut common,
        &mut base_no,
        &mut ours_no,
        &mut theirs_no,
    );

    Ok(MergeResult {
        chunks,
        conflicts,
        ours_changes: 0,
        theirs_changes: 0,
        agree: 0,
        ours_label,
        theirs_label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 提取块类型序列,便于断言整体结构
    fn kinds(result: &MergeResult) -> Vec<ChunkKind> {
        result.chunks.iter().map(|c| c.kind).collect()
    }

    #[test]
    fn identical_inputs_are_stable() {
        let result = merge_text("a\nb\n", "a\nb\n", "a\nb\n");
        assert_eq!(kinds(&result), vec![ChunkKind::Stable]);
        assert_eq!(result.conflicts, 0);
    }

    #[test]
    fn non_overlapping_changes_merge() {
        let result = merge_text("a\nb\nc\nd\n", "A\nb\nc\nd\n", "a\nb\nc\nD\n");
        assert_eq!(
            kinds(&result),
            vec![ChunkKind::Ours, ChunkKind::Stable, ChunkKind::Theirs]
        );
        assert_eq!(result.chunks[0].ours, vec!["A"]);
        assert_eq!(result.chunks[0].base, vec!["a"]);
        assert_eq!(result.chunks[2].theirs, vec!["D"]);
        assert_eq!(result.conflicts, 0);
        assert_eq!(result.ours_changes, 1);
        assert_eq!(result.theirs_changes, 1);
    }

    #[test]
    fn identical_changes_are_agree() {
        let result = merge_text("a\nb\nc\n", "a\nB\nc\n", "a\nB\nc\n");
        assert_eq!(
            kinds(&result),
            vec![ChunkKind::Stable, ChunkKind::Agree, ChunkKind::Stable]
        );
        assert_eq!(result.agree, 1);
        assert_eq!(result.chunks[1].ours, result.chunks[1].theirs);
    }

    #[test]
    fn conflicting_change_detected() {
        let result = merge_text("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        assert_eq!(result.conflicts, 1);
        let conflict = &result.chunks[1];
        assert_eq!(conflict.kind, ChunkKind::Conflict);
        assert_eq!(conflict.base, vec!["b"]);
        assert_eq!(conflict.ours, vec!["X"]);
        assert_eq!(conflict.theirs, vec!["Y"]);
    }

    #[test]
    fn insertions_at_same_point_conflict() {
        let result = merge_text("a\nb\n", "a\nx\nb\n", "a\ny\nb\n");
        assert_eq!(result.conflicts, 1);
        let conflict = &result.chunks[1];
        assert!(conflict.base.is_empty());
        assert_eq!(conflict.ours, vec!["x"]);
        assert_eq!(conflict.theirs, vec!["y"]);
    }

    #[test]
    fn delete_vs_edit_conflicts() {
        let result = merge_text("a\nb\nc\n", "a\nc\n", "a\nB\nc\n");
        assert_eq!(result.conflicts, 1);
        let conflict = &result.chunks[1];
        assert!(conflict.ours.is_empty());
        assert_eq!(conflict.theirs, vec!["B"]);
    }

    #[test]
    fn adjacent_changes_stay_separate() {
        let result = merge_text("a\nb\nc\nd\n", "A\nB\nc\nd\n", "a\nb\nC\nD\n");
        assert_eq!(kinds(&result), vec![ChunkKind::Ours, ChunkKind::Theirs]);
        assert_eq!(result.conflicts, 0);
    }

    #[test]
    fn empty_base_same_addition_agrees() {
        let result = merge_text("", "x\n", "x\n");
        assert_eq!(kinds(&result), vec![ChunkKind::Agree]);
    }

    #[test]
    fn empty_base_different_additions_conflict() {
        let result = merge_text("", "x\n", "y\n");
        assert_eq!(kinds(&result), vec![ChunkKind::Conflict]);
    }

    #[test]
    fn one_side_unchanged_keeps_other() {
        let result = merge_text("a\nb\n", "a\nB\n", "a\nb\n");
        assert_eq!(kinds(&result), vec![ChunkKind::Stable, ChunkKind::Ours]);
        assert_eq!(result.theirs_changes, 0);
    }

    #[test]
    fn line_numbers_track_each_side() {
        // ours 在 a 后插入两行,theirs 修改 c;两处改动互不碰撞
        let result = merge_text("a\nb\nc\n", "a\nx\ny\nb\nc\n", "a\nb\nC\n");
        assert_eq!(
            kinds(&result),
            vec![
                ChunkKind::Stable,
                ChunkKind::Ours,
                ChunkKind::Stable,
                ChunkKind::Theirs
            ]
        );
        let theirs_chunk = &result.chunks[3];
        assert_eq!(theirs_chunk.base_start, 3);
        assert_eq!(theirs_chunk.ours_start, 5);
        assert_eq!(theirs_chunk.theirs_start, 3);
    }

    #[test]
    fn crlf_lines_are_clean() {
        let result = merge_text("a\r\nb\r\n", "a\r\nB\r\n", "a\r\nb\r\n");
        let all_lines = result
            .chunks
            .iter()
            .flat_map(|c| c.base.iter().chain(c.ours.iter()).chain(c.theirs.iter()));
        for line in all_lines {
            assert!(!line.contains('\r'));
        }
    }

    // —— 冲突文件解析 ——

    #[test]
    fn parses_standard_conflict_markers() {
        let text = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> feature/demo\nb\n";
        let result = parse_conflict_file(text).unwrap();
        assert_eq!(
            kinds(&result),
            vec![ChunkKind::Stable, ChunkKind::Conflict, ChunkKind::Stable]
        );
        assert_eq!(result.conflicts, 1);
        assert_eq!(result.ours_label.as_deref(), Some("HEAD"));
        assert_eq!(result.theirs_label.as_deref(), Some("feature/demo"));

        let conflict = &result.chunks[1];
        assert_eq!(conflict.ours, vec!["x"]);
        assert_eq!(conflict.theirs, vec!["y"]);
        assert!(conflict.base.is_empty());
        assert_eq!(conflict.ours_start, 2);
        assert_eq!(conflict.theirs_start, 2);
    }

    #[test]
    fn parses_diff3_style_base_section() {
        let text =
            "<<<<<<< HEAD\nx\n||||||| merged common ancestors\no\n=======\ny\n>>>>>>> main\n";
        let result = parse_conflict_file(text).unwrap();
        let conflict = &result.chunks[0];
        assert_eq!(conflict.kind, ChunkKind::Conflict);
        assert_eq!(conflict.base, vec!["o"]);
        assert_eq!(conflict.ours, vec!["x"]);
        assert_eq!(conflict.theirs, vec!["y"]);
    }

    #[test]
    fn parses_multiple_conflicts() {
        let text = "a\n<<<<<<<\nx\n=======\ny\n>>>>>>>\nb\n<<<<<<<\np\n=======\nq\n>>>>>>>\n";
        let result = parse_conflict_file(text).unwrap();
        assert_eq!(result.conflicts, 2);
        assert!(result.ours_label.is_none());
        // 第二个冲突的行号应累计前面所有内容
        let second = &result.chunks[3];
        assert_eq!(second.ours_start, 4); // a, x, b 之后
        assert_eq!(second.theirs_start, 4); // a, y, b 之后
    }

    #[test]
    fn conflict_file_without_markers_errors() {
        assert!(matches!(
            parse_conflict_file("hello\nworld\n"),
            Err(ConflictParseError::NoMarkers)
        ));
    }

    #[test]
    fn unterminated_conflict_errors() {
        assert!(matches!(
            parse_conflict_file("a\n<<<<<<< HEAD\nx\n=======\ny\n"),
            Err(ConflictParseError::Malformed(2))
        ));
    }

    #[test]
    fn nested_marker_errors() {
        assert!(matches!(
            parse_conflict_file("<<<<<<< HEAD\n<<<<<<< again\n"),
            Err(ConflictParseError::Malformed(2))
        ));
    }

    #[test]
    fn separator_outside_conflict_is_content() {
        // 公共区域里恰好出现 ======= 行(如 RST 标题)不应被当作标记
        let text = "title\n=======\n<<<<<<<\nx\n=======\ny\n>>>>>>>\n";
        let result = parse_conflict_file(text).unwrap();
        assert_eq!(result.chunks[0].base, vec!["title", "======="]);
        assert_eq!(result.conflicts, 1);
    }

    #[test]
    fn conflict_file_with_crlf_is_clean() {
        let text = "a\r\n<<<<<<< HEAD\r\nx\r\n=======\r\ny\r\n>>>>>>> main\r\n";
        let result = parse_conflict_file(text).unwrap();
        let conflict = &result.chunks[1];
        assert_eq!(conflict.ours, vec!["x"]);
        assert_eq!(conflict.theirs, vec!["y"]);
    }
}
