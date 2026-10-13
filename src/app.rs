//! State machine of a conflict-resolution session (pure logic; touches neither the terminal nor git).
//!
//! The model mirrors the frontend semantics of the toolkit-rs web version:
//! each side of a change chunk is in one of three states (pending / applied /
//! ignored), applied content is appended in take order (taking both sides of a
//! conflict one after another means "keep both"), and editing via `$EDITOR`
//! overrides the whole chunk.

use crate::merge::{ChunkKind, MergeChunk, MergeResult, merge_text};

/// Side from which to take a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Local (left pane)
    Ours,
    /// Remote (right pane)
    Theirs,
}

/// Resolution state of one side's change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideState {
    /// Awaiting a decision
    Pending,
    /// Applied to the result
    Applied,
    /// Ignored (excluded from the result)
    Ignored,
}

/// Resolution state of a single chunk.
#[derive(Debug, Clone)]
pub struct ChunkState {
    /// Local side state
    pub ours: SideState,
    /// Remote side state
    pub theirs: SideState,
    /// Take order, which determines content concatenation order
    pub order: Vec<Side>,
    /// Whole-chunk override from manual editing in $EDITOR
    pub override_lines: Option<Vec<String>>,
}

impl ChunkState {
    /// Initial pending state.
    fn new() -> Self {
        Self {
            ours: SideState::Pending,
            theirs: SideState::Pending,
            order: Vec::new(),
            override_lines: None,
        }
    }

    /// Read the state of one side.
    fn side(&self, side: Side) -> SideState {
        match side {
            Side::Ours => self.ours,
            Side::Theirs => self.theirs,
        }
    }

    /// Set the state of one side.
    fn set_side(&mut self, side: Side, state: SideState) {
        match side {
            Side::Ours => self.ours = state,
            Side::Theirs => self.theirs = state,
        }
    }
}

/// Sides with changes: one for single-sided chunks, both for agreement or conflict.
fn effective_sides(kind: ChunkKind) -> &'static [Side] {
    match kind {
        ChunkKind::Ours => &[Side::Ours],
        ChunkKind::Theirs => &[Side::Theirs],
        ChunkKind::Agree | ChunkKind::Conflict => &[Side::Ours, Side::Theirs],
        ChunkKind::Stable => &[],
    }
}

/// Merge session for one text file: chunks, resolution states, and cursor.
#[derive(Debug)]
pub struct FileMerge {
    /// File path (repository-relative in Git mode; input path in file mode)
    pub path: String,
    /// Contiguous merge chunks covering the entire file
    pub chunks: Vec<MergeChunk>,
    /// Resolution states corresponding one-to-one with chunks
    pub states: Vec<ChunkState>,
    /// Cursor chunk index (a non-Stable chunk, or 0 if there are no changes)
    pub cursor: usize,
    /// Vertical viewport offset in rows, managed by the UI
    pub scroll: usize,
    /// Horizontal viewport offset in display columns, shared by all panes and managed by the UI
    pub hscroll: usize,
    /// Whether the viewport follows the cursor; manual scrolling disables it, cursor actions restore it
    pub follow: bool,
    /// Local pane header label (branch name)
    pub ours_label: Option<String>,
    /// Remote pane header label (branch name)
    pub theirs_label: Option<String>,
    /// Append a trailing newline on export if any original input had one
    ends_with_newline: bool,
}

impl FileMerge {
    /// Build from three complete texts (Git three-way mode).
    pub fn from_three_way(path: String, base: &str, ours: &str, theirs: &str) -> Self {
        let ends = base.ends_with('\n') || ours.ends_with('\n') || theirs.ends_with('\n');
        Self::from_result(path, merge_text(base, ours, theirs), ends)
    }

    /// Build from a computed merge result (conflict-file parsing mode).
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
            hscroll: 0,
            follow: true,
            ours_label: result.ours_label,
            theirs_label: result.theirs_label,
            chunks: result.chunks,
            ends_with_newline,
        }
    }

    // Queries

    /// Whether a chunk is overridden or all its effective sides have been handled.
    pub fn chunk_resolved(&self, idx: usize) -> bool {
        let state = &self.states[idx];
        if state.override_lines.is_some() {
            return true;
        }
        effective_sides(self.chunks[idx].kind)
            .iter()
            .all(|&side| state.side(side) != SideState::Pending)
    }

    /// Number of unresolved conflict chunks; must be zero before writing.
    pub fn pending_conflicts(&self) -> usize {
        (0..self.chunks.len())
            .filter(|&i| self.chunks[i].kind == ChunkKind::Conflict && !self.chunk_resolved(i))
            .count()
    }

    /// Number of unresolved changes, including non-conflicts, for the status bar.
    pub fn pending_changes(&self) -> usize {
        (0..self.chunks.len())
            .filter(|&i| self.chunks[i].kind != ChunkKind::Stable && !self.chunk_resolved(i))
            .count()
    }

    /// Whether all conflicts are resolved; pending non-conflict chunks retain base content.
    pub fn ready_to_write(&self) -> bool {
        self.pending_conflicts() == 0
    }

    /// Current chunk content: override > concatenation in take order > base.
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

    /// Export the final file content by joining chunks and applying the trailing-newline policy.
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

    // Operations on the cursor's chunk

    /// Take one side, appending content in take order; taking both sides keeps both.
    /// For agreement chunks, taking either identical side resolves the whole chunk.
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

    /// Ignore a side's change; ignoring an agreement chunk retains its base content.
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

    /// Undo all decisions for the current chunk, restoring its pending state.
    pub fn undo(&mut self) {
        self.states[self.cursor] = ChunkState::new();
    }

    /// Undo all file decisions, including editor overrides, restoring pending states.
    pub fn undo_all(&mut self) {
        for state in &mut self.states {
            *state = ChunkState::new();
        }
    }

    /// Override the current chunk with editor content and mark it resolved.
    pub fn set_override(&mut self, lines: Vec<String>) {
        self.states[self.cursor].override_lines = Some(lines);
    }

    /// Apply all non-conflicting changes (ours / theirs / agreement).
    pub fn apply_all_nonconflict(&mut self) {
        let saved = self.cursor;
        for idx in 0..self.chunks.len() {
            let kind = self.chunks[idx].kind;
            if matches!(kind, ChunkKind::Stable | ChunkKind::Conflict) || self.chunk_resolved(idx) {
                continue;
            }
            self.cursor = idx;
            // The first effective side supplies the change; either side works for agreement.
            if let Some(&side) = effective_sides(kind).first() {
                self.apply(side);
            }
        }
        self.cursor = saved;
    }

    // Cursor movement

    /// Move to the next changed chunk, skipping Stable chunks.
    pub fn next_change(&mut self) {
        if let Some(idx) =
            (self.cursor + 1..self.chunks.len()).find(|&i| self.chunks[i].kind != ChunkKind::Stable)
        {
            self.cursor = idx;
        }
    }

    /// Move to the previous changed chunk, skipping Stable chunks.
    pub fn prev_change(&mut self) {
        if let Some(idx) = (0..self.cursor)
            .rev()
            .find(|&i| self.chunks[i].kind != ChunkKind::Stable)
        {
            self.cursor = idx;
        }
    }

    /// Jump to the next unresolved conflict, wrapping at the end.
    pub fn next_conflict(&mut self) {
        let n = self.chunks.len();
        if let Some(idx) = (1..=n)
            .map(|step| (self.cursor + step) % n)
            .find(|&i| self.chunks[i].kind == ChunkKind::Conflict && !self.chunk_resolved(i))
        {
            self.cursor = idx;
        }
    }

    /// Jump to the previous unresolved conflict, wrapping at the start.
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

/// Object type for a whole-file pick, determining how each side is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    /// Regular file (binary or deleted on one side): display byte count
    File,
    /// Symlink: display the link target
    Symlink,
    /// Submodule: display the recorded commit ID
    Submodule,
}

/// One file awaiting resolution in a session.
#[derive(Debug)]
pub enum FileEntry {
    /// Text file: resolve chunk by chunk in three panes
    Text(FileMerge),
    /// Conflicts that cannot be merged by chunk (binary / symlink / submodule / modify-delete)
    /// fall back to choosing one whole side
    Pick {
        /// File path
        path: String,
        /// Object type
        kind: PickKind,
        /// Complete local content (link target or commit ID for special objects); None means deleted
        ours: Option<Vec<u8>>,
        /// Complete remote content; None means deleted
        theirs: Option<Vec<u8>>,
        /// User's choice
        choice: Option<Side>,
    },
}

impl FileEntry {
    /// File path.
    pub fn path(&self) -> &str {
        match self {
            FileEntry::Text(m) => &m.path,
            FileEntry::Pick { path, .. } => path,
        }
    }

    /// Whether the file is ready to write.
    pub fn ready_to_write(&self) -> bool {
        match self {
            FileEntry::Text(m) => m.ready_to_write(),
            FileEntry::Pick { choice, .. } => choice.is_some(),
        }
    }

    /// Export final bytes when ready_to_write; empty if the chosen side is deleted.
    ///
    /// Git mode persists whole-file picks through Git, deleting the file for a deleted
    /// side rather than writing an empty file; see `commands::resolve::persist`.
    pub fn resolved_bytes(&self) -> Vec<u8> {
        match self {
            FileEntry::Text(m) => m.resolved_content().into_bytes(),
            FileEntry::Pick {
                ours,
                theirs,
                choice,
                ..
            } => match choice {
                Some(Side::Theirs) => theirs.clone().unwrap_or_default(),
                _ => ours.clone().unwrap_or_default(),
            },
        }
    }
}

/// Conflict-resolution session spanning multiple files.
#[derive(Debug)]
pub struct Session {
    /// All files awaiting resolution
    pub files: Vec<FileEntry>,
    /// Current file index
    pub current: usize,
    /// Whether each file has been written
    pub written: Vec<bool>,
    /// Operation label for the status bar, such as "merge", "rebase", or a filename
    pub op_label: String,
    /// Whether long stable regions are folded (toggled with z)
    pub folded: bool,
}

impl Session {
    /// Create a session with the cursor on the first file.
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

    /// Current file.
    pub fn current_file(&self) -> &FileEntry {
        &self.files[self.current]
    }

    /// Mutable access to the current file.
    pub fn current_file_mut(&mut self) -> &mut FileEntry {
        &mut self.files[self.current]
    }

    /// Mark the current file written and select an unwritten file if any remain.
    pub fn mark_written(&mut self) {
        self.written[self.current] = true;
        if let Some(idx) = (0..self.files.len()).find(|&i| !self.written[i]) {
            self.current = idx;
        }
    }

    /// Whether all files have been written.
    pub fn all_written(&self) -> bool {
        self.written.iter().all(|&w| w)
    }

    /// Switch to the next file, wrapping at the end.
    pub fn next_file(&mut self) {
        if !self.files.is_empty() {
            self.current = (self.current + 1) % self.files.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a session file with [Stable, Conflict, Stable, Ours] chunks.
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
        // The cursor starts on the first changed chunk.
        assert_eq!(merge.cursor, 1);
    }

    #[test]
    fn apply_ours_resolves_conflict_side() {
        let mut merge = sample();
        merge.apply(Side::Ours);
        assert!(!merge.chunk_resolved(1)); // Theirs is still pending.
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
        // Taking a side has no effect after an override.
        merge.apply(Side::Ours);
        assert_eq!(merge.current_content(1), vec!["merged"]);
    }

    #[test]
    fn apply_on_ineffective_side_is_noop() {
        let mut merge = sample();
        merge.cursor = 3; // Ours chunk
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
        // Two conflicts: chunks 1 and 3.
        assert_eq!(merge.pending_conflicts(), 2);
        assert_eq!(merge.cursor, 1);
        merge.next_conflict();
        assert_eq!(merge.cursor, 3);
        merge.next_conflict();
        assert_eq!(merge.cursor, 1); // Wrap around.
        // After resolving chunk 1, navigate only among remaining conflicts.
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
            FileEntry::Pick {
                path: "logo.png".to_owned(),
                kind: PickKind::File,
                ours: Some(vec![1, 2]),
                theirs: Some(vec![3, 4]),
                choice: None,
            },
        ];
        let mut session = Session::new(files, "merge".to_owned());
        assert!(!session.all_written());
        session.mark_written();
        assert_eq!(session.current, 1);
        // A whole-file pick is ready to write once a side is chosen.
        if let FileEntry::Pick { choice, .. } = session.current_file_mut() {
            *choice = Some(Side::Theirs);
        }
        assert!(session.current_file().ready_to_write());
        assert_eq!(session.current_file().resolved_bytes(), vec![3, 4]);
        session.mark_written();
        assert!(session.all_written());
    }
}
