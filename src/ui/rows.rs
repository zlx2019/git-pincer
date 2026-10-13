//! Render-row structures and construction: align chunks across three panes, with folding and placeholders.

use crate::app::{FileMerge, SideState};
use crate::merge::{ChunkKind, MergeChunk};

/// Stable-region folding threshold and retained context lines, matching the web version.
pub(crate) const FOLD_THRESHOLD: usize = 8;
pub(crate) const FOLD_KEEP: usize = 3;

/// Change type controlling band colors: blue=modified, green=added, gray=deleted, red=conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeType {
    /// Neither side changed
    None,
    /// Existing lines modified
    Modified,
    /// Lines added (absent from base)
    Added,
    /// Lines deleted (the changed side is empty)
    Deleted,
    /// Conflicting changes from both sides
    Conflict,
}

/// Infer change type from chunk kind and content; classify agreement as a regular non-conflict change.
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
        ChunkKind::Ours => of(chunk.ours_lines()),
        ChunkKind::Theirs => of(chunk.theirs_lines()),
        ChunkKind::Agree => of(chunk.ours_lines()),
    }
}

/// One cell in a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell {
    /// Regular content line
    Line {
        /// Absolute line number in this pane (1-based)
        no: usize,
        /// Line offset within the chunk for word emphasis lookup
        offset: usize,
        /// Line text without its newline
        text: String,
    },
    /// No content in this pane's row because chunk sides have unequal lengths
    Empty,
    /// Result-pane placeholder for an unresolved conflict chunk
    Placeholder,
}

/// One render row: a cell per pane, or a folding notice spanning all panes.
///
/// Cursor selection is computed during rendering by comparing `chunk` with the
/// cursor, allowing rows to be cached by revision across frequent navigation changes.
#[derive(Debug)]
pub(crate) struct Row {
    /// Owning chunk index for emphasis lookup and cursor selection
    pub(crate) chunk: usize,
    /// Chunk change type for band and gutter colors
    pub(crate) change: ChangeType,
    pub(crate) resolved: bool,
    /// Folded row count; the rendering layer generates the display text
    pub(crate) fold: Option<usize>,
    /// Local resolution state; None means no local change in this chunk
    pub(crate) ours_state: Option<SideState>,
    /// Remote resolution state; None means no remote change in this chunk
    pub(crate) theirs_state: Option<SideState>,
    pub(crate) ours: Cell,
    pub(crate) result: Cell,
    pub(crate) theirs: Cell,
}

/// Render rows cached by (file, revision, folding state).
///
/// Expanding rows clones strings across the file in O(line count). Caching avoids
/// rebuilds during navigation and scrolling; only content revisions and folding
/// changes trigger recomputation.
#[derive(Debug, Default)]
pub(crate) struct RowCache {
    /// Cache key: (file index, state revision, folding state)
    key: Option<(usize, u64, bool)>,
    rows: Vec<Row>,
    chunk_starts: Vec<usize>,
    max_no: usize,
}

impl RowCache {
    /// Get cached rows, returning (rows, first row index per chunk, maximum file line number).
    pub(crate) fn get(
        &mut self,
        file_idx: usize,
        merge: &FileMerge,
        rev: u64,
        folded: bool,
    ) -> (&[Row], &[usize], usize) {
        let key = (file_idx, rev, folded);
        if self.key != Some(key) {
            let (rows, starts) = build_rows(merge, folded);
            // Use the maximum file line number to keep the number column stable while scrolling.
            self.max_no = rows
                .iter()
                .flat_map(|r| [&r.ours, &r.result, &r.theirs])
                .filter_map(|c| match c {
                    Cell::Line { no, .. } => Some(*no),
                    _ => None,
                })
                .max()
                .unwrap_or(1);
            self.rows = rows;
            self.chunk_starts = starts;
            self.key = Some(key);
        }
        (&self.rows, &self.chunk_starts, self.max_no)
    }
}

/// Expand file chunks into render rows and return each chunk's first row index for scrolling.
pub(crate) fn build_rows(merge: &FileMerge, folded: bool) -> (Vec<Row>, Vec<usize>) {
    let mut rows: Vec<Row> = Vec::new();
    let mut chunk_starts: Vec<usize> = Vec::new();
    let mut result_no = 1usize;

    for (idx, chunk) in merge.chunks.iter().enumerate() {
        chunk_starts.push(rows.len());
        let resolved = merge.chunk_resolved(idx);
        let change = change_type(chunk);
        let result_lines = merge.current_content(idx);
        // Side states for gutter symbols; single-side chunks only expose that side.
        let st = &merge.states[idx];
        let (ours_state, theirs_state) = match chunk.kind {
            ChunkKind::Ours => (Some(st.ours), None),
            ChunkKind::Theirs => (None, Some(st.theirs)),
            ChunkKind::Agree | ChunkKind::Conflict => (Some(st.ours), Some(st.theirs)),
            ChunkKind::Stable => (None, None),
        };

        let height = chunk
            .ours_lines()
            .len()
            .max(result_lines.len())
            .max(chunk.theirs_lines().len())
            .max(1);
        // Unresolved conflicts show a centered pending placeholder instead of base in the result pane.
        let placeholder_at = (chunk.kind == ChunkKind::Conflict && !resolved).then_some(height / 2);

        // Generate aligned rows across all panes; i is the offset within the chunk.
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
                    fold: None,
                    ours_state,
                    theirs_state,
                    ours: cell(chunk.ours_lines(), chunk.ours_start),
                    result: match placeholder_at {
                        Some(at) if i == at => Cell::Placeholder,
                        Some(_) => Cell::Empty,
                        None => cell(&result_lines, result_no),
                    },
                    theirs: cell(chunk.theirs_lines(), chunk.theirs_start),
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

    /// Folded rows and line numbers are constructed correctly.
    #[test]
    fn build_rows_folds_long_stable_chunks() {
        let base: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let merge = FileMerge::from_three_way("demo.txt".to_owned(), &base, &base, &base);
        let (rows, starts) = build_rows(&merge, true);
        // Fold 20 stable lines into 3 context lines + a fold row + 3 context lines.
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[3].fold, Some(14));
        assert_eq!(starts, vec![0]);

        let (unfolded, _) = build_rows(&merge, false);
        assert_eq!(unfolded.len(), 20);
    }

    /// Infer added / deleted / modified / conflicting changes using IDEA color semantics.
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
        // Single-side addition (empty base).
        let c = mk(ChunkKind::Ours, &[], &["new"], &[]);
        assert_eq!(change_type(&c), ChangeType::Added);
        // Single-side deletion (empty changed side).
        let c = mk(ChunkKind::Theirs, &["old"], &["old"], &[]);
        assert_eq!(change_type(&c), ChangeType::Deleted);
        // Single-side modification.
        let c = mk(ChunkKind::Ours, &["a"], &["b"], &["a"]);
        assert_eq!(change_type(&c), ChangeType::Modified);
        // Conflicts always remain conflicts.
        let c = mk(ChunkKind::Conflict, &[], &["x"], &["y"]);
        assert_eq!(change_type(&c), ChangeType::Conflict);
        // Classify agreement by the nature of the change.
        let c = mk(ChunkKind::Agree, &[], &["n"], &["n"]);
        assert_eq!(change_type(&c), ChangeType::Added);
    }

    /// Reuse rows for matching cache keys; rebuild when revision or folded changes.
    #[test]
    fn row_cache_hits_and_invalidates() {
        let base: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let mut merge =
            FileMerge::from_three_way("demo.txt".to_owned(), &base, &base, &format!("{base}x\n"));
        let mut cache = RowCache::default();

        let ptr = cache.get(0, &merge, 0, true).0.as_ptr();
        // Reusing the same key does not rebuild (the slice address stays unchanged).
        assert_eq!(cache.get(0, &merge, 0, true).0.as_ptr(), ptr);

        // Changing folded rebuilds rows and expands the 20-line stable region.
        let folded_len = cache.get(0, &merge, 0, true).0.len();
        let unfolded_len = cache.get(0, &merge, 0, false).0.len();
        assert!(unfolded_len > folded_len);

        // Changing revision rebuilds rows to reflect the latest taken content.
        merge.apply(Side::Theirs);
        let (rows, _, _) = cache.get(0, &merge, 1, false);
        assert!(rows.iter().any(|r| matches!(
            &r.result,
            Cell::Line { text, .. } if text == "x"
        )));
    }

    /// Unresolved result chunks show placeholders, replaced by content after resolution.
    #[test]
    fn result_shows_placeholder_until_conflict_resolved() {
        let mut merge =
            FileMerge::from_three_way("demo.txt".to_owned(), "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        let (rows, _) = build_rows(&merge, false);
        assert!(rows.iter().any(|r| r.result == Cell::Placeholder));

        // Taking local and ignoring remote resolves the conflict and replaces the placeholder.
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
