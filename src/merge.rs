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

/// Maximum time per diff; similar falls back to coarser results on timeout,
/// keeping TUI startup responsive even for very large files.
const DIFF_TIMEOUT: Duration = Duration::from_millis(500);

/// Merge chunk classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// Neither side changed; content matches base
    Stable,
    /// Only local (ours) changed; applies without conflict
    Ours,
    /// Only remote (theirs) changed; applies without conflict
    Theirs,
    /// Both sides made identical changes; applies without conflict
    Agree,
    /// Both sides made different changes; requires manual resolution
    Conflict,
}

/// A contiguous base range and both sides' content for that range.
#[derive(Debug, Clone)]
pub struct MergeChunk {
    /// Chunk index used by the interactive layer
    pub id: usize,
    /// Chunk classification
    pub kind: ChunkKind,
    /// Base lines in this range, without line endings
    pub base: Vec<String>,
    /// Local lines in this range; empty for stable chunks to avoid duplication.
    /// Read through [`Self::ours_lines`]
    pub ours: Vec<String>,
    /// Remote lines in this range; empty for stable chunks to avoid duplication.
    /// Read through [`Self::theirs_lines`]
    pub theirs: Vec<String>,
    /// Starting base line number (1-based)
    pub base_start: usize,
    /// Starting local line number (1-based)
    pub ours_start: usize,
    /// Starting remote line number (1-based)
    pub theirs_start: usize,
}

impl MergeChunk {
    /// Local lines; stable chunks share base storage because both sides are identical,
    /// avoiding three copies of unchanged file content.
    pub fn ours_lines(&self) -> &[String] {
        if self.kind == ChunkKind::Stable {
            &self.base
        } else {
            &self.ours
        }
    }

    /// Remote lines; stable chunks share base storage because both sides are identical.
    pub fn theirs_lines(&self) -> &[String] {
        if self.kind == ChunkKind::Stable {
            &self.base
        } else {
            &self.theirs
        }
    }
}

/// Complete three-way merge result: chunks in file order and statistics.
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// Contiguous merge chunks covering the entire file
    pub chunks: Vec<MergeChunk>,
    /// Number of conflict chunks
    pub conflicts: usize,
    /// Number of local-only change chunks
    pub ours_changes: usize,
    /// Number of remote-only change chunks
    pub theirs_changes: usize,
    /// Number of identical-change chunks
    pub agree: usize,
    /// Local label from the branch name after `<<<<<<<`; None in three-way mode
    pub ours_label: Option<String>,
    /// Remote label from the branch name after `>>>>>>>`; None in three-way mode
    pub theirs_label: Option<String>,
}

/// A single-side hunk replacing a base line range with new lines.
#[derive(Debug, Clone)]
struct Hunk {
    /// Replaced half-open base range; empty for a pure insertion
    base_range: Range<usize>,
    /// Replacement lines without line endings
    lines: Vec<String>,
}

/// Side that produced a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Ours,
    Theirs,
}

/// A side-tagged hunk, allowing both sides to be sorted together for grouping.
#[derive(Debug, Clone)]
struct SidedHunk {
    side: Side,
    hunk: Hunk,
}

/// Merge three texts into chunks in file order, with statistics.
pub fn merge_text(base: &str, ours: &str, theirs: &str) -> MergeResult {
    let ours_diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DIFF_TIMEOUT)
        .diff_lines(base, ours);
    let theirs_diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DIFF_TIMEOUT)
        .diff_lines(base, theirs);

    // Use similar's base line boundaries to keep operation range indices aligned.
    let base_lines: Vec<String> = (0..ours_diff.old_len())
        .filter_map(|i| ours_diff.old_slice(i))
        .map(clean_line)
        .collect();

    let groups = group_hunks(extract_hunks(&ours_diff), extract_hunks(&theirs_diff));
    build_chunks(&base_lines, groups)
}

/// Strip line endings (\n or \r\n).
fn clean_line(line: &str) -> String {
    line.trim_end_matches(['\r', '\n']).to_owned()
}

/// Extract all non-Equal operations from a two-way diff as hunks.
fn extract_hunks(diff: &TextDiff<'_, '_, str>) -> Vec<Hunk> {
    diff.ops()
        .iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| Hunk {
            base_range: op.old_range(),
            // new_slice exists within valid ranges; filter_map avoids unwrap.
            lines: op
                .new_range()
                .filter_map(|i| diff.new_slice(i))
                .map(clean_line)
                .collect(),
        })
        .collect()
}

/// Whether two base ranges collide and must be grouped together.
///
/// Conservatively treat an insertion touching another range's endpoint as a collision,
/// preferring an extra conflict over an incorrect automatic merge. Nonempty ranges
/// that only share an endpoint remain independent.
fn collides(a: &Range<usize>, b: &Range<usize>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => a.start == b.start,
        (true, false) => b.start <= a.start && a.start <= b.end,
        (false, true) => a.start <= b.start && b.start <= a.end,
        (false, false) => a.start.max(b.start) < a.end.min(b.end),
    }
}

/// Group both sides' hunks by base-range collisions, ordered by base start within groups.
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
    // Sort by base start, placing ours first for matching ranges to keep results deterministic.
    all.sort_by_key(|s| {
        (
            s.hunk.base_range.start,
            s.hunk.base_range.end,
            s.side == Side::Theirs,
        )
    });

    // Sorted hunks let each group's union range expand transitively on collisions.
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

/// Compute one side's final content for the combined base range [lo, hi):
/// preserve untouched base lines and substitute replacement lines for its hunks.
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

/// Build contiguous merge chunks from hunk groups and count each chunk kind.
fn build_chunks(base_lines: &[String], groups: Vec<Vec<SidedHunk>>) -> MergeResult {
    let mut chunks: Vec<MergeChunk> = Vec::new();
    let mut conflicts = 0usize;
    let mut ours_changes = 0usize;
    let mut theirs_changes = 0usize;
    let mut agree = 0usize;

    // Per-side line cursors (1-based) and the consumed base position.
    let mut base_pos = 0usize;
    let mut base_no = 1usize;
    let mut ours_no = 1usize;
    let mut theirs_no = 1usize;

    // Append a stable chunk, storing only base because both sides match it.
    let push_stable = |chunks: &mut Vec<MergeChunk>,
                       lines: &[String],
                       base_no: &mut usize,
                       ours_no: &mut usize,
                       theirs_no: &mut usize| {
        chunks.push(MergeChunk {
            id: chunks.len(),
            kind: ChunkKind::Stable,
            base: lines.to_vec(),
            ours: Vec::new(),
            theirs: Vec::new(),
            base_start: *base_no,
            ours_start: *ours_no,
            theirs_start: *theirs_no,
        });
        *base_no += lines.len();
        *ours_no += lines.len();
        *theirs_no += lines.len();
    };

    for group in groups {
        // Combined base range of a nonempty group; unwrap_or avoids unwrap.
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

        // Stable region preceding this group.
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

        let (ours_len, theirs_len) = (ours_lines.len(), theirs_lines.len());
        chunks.push(MergeChunk {
            id: chunks.len(),
            kind,
            base: base_lines[lo..hi].to_vec(),
            ours: ours_lines,
            theirs: theirs_lines,
            base_start: base_no,
            ours_start: ours_no,
            theirs_start: theirs_no,
        });
        base_no += hi - lo;
        ours_no += ours_len;
        theirs_no += theirs_len;
        base_pos = hi;
    }

    // Trailing stable region.
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

/// Conflict-file parsing error.
#[derive(Debug, thiserror::Error)]
pub enum ConflictParseError {
    /// No conflict markers anywhere in the file
    #[error("未检测到 git 冲突标记(<<<<<<< / ======= / >>>>>>>)")]
    NoMarkers,
    /// Incomplete or out-of-order conflict markers
    #[error("第 {0} 行附近的冲突标记不完整或顺序错误")]
    Malformed(usize),
}

/// Conflict section currently being collected by the parser.
enum Section {
    /// Common content outside conflict chunks
    Common,
    /// Local content after `<<<<<<<`
    Ours,
    /// Base content after `|||||||` (diff3 style only)
    Base,
    /// Remote content after `=======`
    Theirs,
}

/// Parse a file with Git conflict markers into merge chunks.
///
/// Git has already applied non-conflicting changes, so only Stable and Conflict
/// chunks remain. Supports default (merge) and diff3 conflictStyle, extracting
/// labels after `<<<<<<< / >>>>>>>`. Default-style conflicts have no base section.
pub fn parse_conflict_file(text: &str) -> Result<MergeResult, ConflictParseError> {
    let mut chunks: Vec<MergeChunk> = Vec::new();
    let mut conflicts = 0usize;
    let mut ours_label: Option<String> = None;
    let mut theirs_label: Option<String> = None;

    // Per-side line cursors (1-based) track positions in the reconstructed documents.
    let mut base_no = 1usize;
    let mut ours_no = 1usize;
    let mut theirs_no = 1usize;

    // Line buffers for each section.
    let mut common: Vec<String> = Vec::new();
    let mut ours: Vec<String> = Vec::new();
    let mut base: Vec<String> = Vec::new();
    let mut theirs: Vec<String> = Vec::new();
    let mut section = Section::Common;
    let mut conflict_start = 0usize;

    // Flush buffered common content into a stable chunk, storing only base.
    let flush_common = |chunks: &mut Vec<MergeChunk>,
                        common: &mut Vec<String>,
                        base_no: &mut usize,
                        ours_no: &mut usize,
                        theirs_no: &mut usize| {
        if common.is_empty() {
            return;
        }
        let lines = std::mem::take(common);
        let len = lines.len();
        chunks.push(MergeChunk {
            id: chunks.len(),
            kind: ChunkKind::Stable,
            base: lines,
            ours: Vec::new(),
            theirs: Vec::new(),
            base_start: *base_no,
            ours_start: *ours_no,
            theirs_start: *theirs_no,
        });
        *base_no += len;
        *ours_no += len;
        *theirs_no += len;
    };

    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        let line_no = idx + 1;

        if let Some(rest) = line.strip_prefix("<<<<<<<") {
            // Start of a conflict; nested <<<<<<< markers are malformed.
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
            let lens = (base_lines.len(), ours_lines.len(), theirs_lines.len());
            chunks.push(MergeChunk {
                id: chunks.len(),
                kind: ChunkKind::Conflict,
                base_start: base_no,
                ours_start: ours_no,
                theirs_start: theirs_no,
                base: base_lines,
                ours: ours_lines,
                theirs: theirs_lines,
            });
            base_no += lens.0;
            ours_no += lens.1;
            theirs_no += lens.2;
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

    // Reaching EOF inside a conflict means the markers are incomplete.
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

    /// Extract chunk kinds for assertions on the overall structure.
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
        // Ours inserts two lines after a; theirs changes c. The changes do not collide.
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

    // Conflict-file parsing

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
        // The second conflict's line numbers include all preceding content.
        let second = &result.chunks[3];
        assert_eq!(second.ours_start, 4); // After a, x, b.
        assert_eq!(second.theirs_start, 4); // After a, y, b.
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
        // A ======= line in common content, such as an RST heading, is not a marker.
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
