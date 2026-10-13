//! A thin wrapper around the native git CLI.
//!
//! All git interaction is done by shelling out (the same route lazygit / IDEA
//! take), so everything the user has already configured is inherited:
//! credentials, hooks, merge strategies, rerere, and so on. Arguments are
//! always passed as arrays and never go through a shell, ruling out injection
//! by construction; commands that may create commits run with
//! `GIT_EDITOR=true` so no editor pops up and hangs the TUI.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};

/// Git command execution failed error
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// Cannot find the git command
    #[error("Cannot find the git executable file, please confirm it is installed and in the PATH.")]
    NotFound,
    /// The specified directory is not a Git repository
    #[error("The specified path is not a Git repository")]
    NotARepo,
    /// The git command execution failed.
    #[error("git {cmd} execution failed: {stderr}")]
    Failed {
        /// Failed subcommands (excluding git prefix)
        cmd: String,
        /// Error message of command output
        stderr: String,
    },
    /// Underlying IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Repository operation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoState {
    /// No merge operation in progress
    Clean,
    /// Merge in progress (MERGE_HEAD exists)
    Merging,
    /// Rebase in progress (rebase-merge, or rebase-apply without an applying marker)
    Rebasing,
    /// Cherry-pick in progress (CHERRY_PICK_HEAD exists)
    CherryPicking,
    /// Revert in progress (REVERT_HEAD exists)
    Reverting,
    /// git am patch application in progress (rebase-apply/applying exists)
    Am,
}

impl RepoState {
    /// Corresponding Git subcommand for `--continue/--abort` and messages.
    pub fn op_name(self) -> &'static str {
        match self {
            RepoState::Clean => "clean",
            RepoState::Merging => "merge",
            RepoState::Rebasing => "rebase",
            RepoState::CherryPicking => "cherry-pick",
            RepoState::Reverting => "revert",
            RepoState::Am => "am",
        }
    }
}

/// Index mode for symbolic links.
const MODE_SYMLINK: u32 = 0o120000;
/// Index mode for submodules (gitlinks).
const MODE_GITLINK: u32 = 0o160000;

/// Index entry for one stage of a conflicted file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// File mode (0o100644 regular file, 0o120000 symlink, 0o160000 submodule)
    pub mode: u32,
    /// Object ID: blob for regular files and symlinks, commit for submodules
    pub oid: String,
}

/// A conflicted file and its index stages.
///
/// Each stage stores a mode and OID, or None when absent, allowing
/// [`Git::read_blobs`] to read content in a single process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictedFile {
    /// Path relative to the repository root
    pub path: String,
    /// Stage 1 (base); absent for add/add conflicts
    pub base: Option<Stage>,
    /// Stage 2 (ours); absent if our side deleted the file
    pub ours: Option<Stage>,
    /// Stage 3 (theirs); absent if their side deleted the file
    pub theirs: Option<Stage>,
}

impl ConflictedFile {
    /// Whether any stage is a symlink. Link targets require a whole-side pick:
    /// text merging or filesystem writes could modify the linked file.
    pub fn is_symlink(&self) -> bool {
        self.has_mode(MODE_SYMLINK)
    }

    /// Whether any stage is a gitlink. Its commit belongs to the submodule's
    /// object database, so resolution picks one side's recorded commit.
    pub fn is_submodule(&self) -> bool {
        self.has_mode(MODE_GITLINK)
    }

    /// Whether any stage has the given mode.
    fn has_mode(&self, mode: u32) -> bool {
        [&self.base, &self.ours, &self.theirs]
            .into_iter()
            .flatten()
            .any(|stage| stage.mode == mode)
    }
}

/// Repository stats queried when opening the menu for its RPG status panel.
#[derive(Debug, Clone)]
pub struct RepoVitals {
    /// Current branch name ("HEAD" when detached)
    pub branch: String,
    /// Number of changed entries in the worktree and index
    pub changes: usize,
    /// Number of stash entries
    pub stashes: usize,
    /// Commits ahead of upstream; None when no upstream is configured
    pub ahead: Option<usize>,
    /// Total commits reachable from HEAD (0 for an empty repository)
    pub level: usize,
}

/// Switchable branches, grouped into local and remote lists for the picker.
#[derive(Debug, Default)]
pub struct SwitchBranches {
    /// Local branches excluding the current branch
    pub locals: Vec<String>,
    /// Remote tracking branches excluding symbolic HEAD refs and existing local counterparts
    pub remotes: Vec<String>,
}

/// Git command context, anchored at the repository root and echoing commands when verbose.
pub struct Git {
    /// Repository root; all commands use `-C` and paths are root-relative
    top: PathBuf,
    /// Whether to echo executed Git commands (-v)
    verbose: bool,
}

/// Convert process startup errors into GitError.
fn spawn_err(e: std::io::Error) -> GitError {
    if e.kind() == std::io::ErrorKind::NotFound {
        GitError::NotFound
    } else {
        GitError::Io(e)
    }
}

/// Git variables to remove from child-process environments.
///
/// Git hooks, including pre-commit test runs, inject paths pointing to the host
/// repository. Remove these variables so nested Git commands cannot be redirected
/// to that repository.
const SCRUBBED_GIT_ENV: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
];

/// Build a Git command at the given directory, scrubbing host paths and disabling the editor.
fn base_git(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).env("GIT_EDITOR", "true");
    for var in SCRUBBED_GIT_ENV {
        cmd.env_remove(var);
    }
    cmd
}

impl Git {
    /// Discover the repository root from a directory and build its command context.
    pub fn discover(dir: &Path, verbose: bool) -> Result<Self, GitError> {
        let out = base_git(dir)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(spawn_err)?;
        if !out.status.success() {
            return Err(GitError::NotARepo);
        }
        let top = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        Ok(Self {
            top: PathBuf::from(top),
            verbose,
        })
    }

    /// Repository root directory.
    pub fn top(&self) -> &Path {
        &self.top
    }

    /// Run Git with captured output; the caller checks the exit code.
    pub fn run(&self, args: &[&str]) -> Result<Output, GitError> {
        if self.verbose {
            eprintln!("[git] git {}", args.join(" "));
        }
        base_git(&self.top).args(args).output().map_err(spawn_err)
    }

    /// Run Git, treating a nonzero exit code as an error.
    pub fn run_ok(&self, args: &[&str]) -> Result<Output, GitError> {
        let out = self.run(args)?;
        if !out.status.success() {
            return Err(GitError::Failed {
                cmd: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(out)
    }

    /// Run Git with output inherited by the user's terminal, preserving progress
    /// display when launching merge / rebase / pull.
    pub fn run_inherit(&self, args: &[&str]) -> Result<ExitStatus, GitError> {
        if self.verbose {
            eprintln!("[git] git {}", args.join(" "));
        }
        base_git(&self.top).args(args).status().map_err(spawn_err)
    }

    /// Detect the repository's current operation state.
    pub fn state(&self) -> Result<RepoState, GitError> {
        let out = self.run_ok(&["rev-parse", "--git-dir"])?;
        let raw = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        // --git-dir may return a path relative to the repository root.
        let git_dir = {
            let p = PathBuf::from(&raw);
            if p.is_absolute() { p } else { self.top.join(p) }
        };
        // Detect rebase before cherry-pick: interactive rebase replays commits and may
        // leave CHERRY_PICK_HEAD on conflict, but must finish with rebase --continue.
        if git_dir.join("rebase-apply").exists() {
            // Git itself uses rebase-apply/applying to distinguish am from rebase.
            if git_dir.join("rebase-apply/applying").exists() {
                Ok(RepoState::Am)
            } else {
                Ok(RepoState::Rebasing)
            }
        } else if git_dir.join("rebase-merge").exists() {
            Ok(RepoState::Rebasing)
        } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
            Ok(RepoState::CherryPicking)
        } else if git_dir.join("REVERT_HEAD").exists() {
            Ok(RepoState::Reverting)
        } else if git_dir.join("MERGE_HEAD").exists() {
            Ok(RepoState::Merging)
        } else {
            Ok(RepoState::Clean)
        }
    }

    /// List all conflicted files.
    pub fn conflicted_files(&self) -> Result<Vec<ConflictedFile>, GitError> {
        let out = self.run_ok(&["ls-files", "-u", "-z"])?;
        Ok(parse_ls_files_unmerged(&String::from_utf8_lossy(
            &out.stdout,
        )))
    }

    /// Read a conflicted file's complete stage content (1=base, 2=ours, 3=theirs).
    pub fn read_stage(&self, path: &str, stage: u8) -> Result<Vec<u8>, GitError> {
        let spec = format!(":{stage}:{path}");
        Ok(self.run_ok(&["show", &spec])?.stdout)
    }

    /// Read blobs through one `git cat-file --batch` process, returning them in request order.
    ///
    /// A separate `git show` per stage costs about 12-16 ms per process, delaying
    /// TUI startup for many conflicts. Reusing one request-response process reduces
    /// process startup overhead from O(3N) to O(1).
    pub fn read_blobs(&self, oids: &[&str]) -> Result<Vec<Vec<u8>>, GitError> {
        use std::io::{BufRead, BufReader, Read, Write};

        if oids.is_empty() {
            return Ok(Vec::new());
        }
        if self.verbose {
            eprintln!("[git] git cat-file --batch ({} blobs)", oids.len());
        }
        let mut child = base_git(&self.top)
            .args(["cat-file", "--batch"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(spawn_err)?;
        // Alternate requests and responses to avoid deadlock from filling both pipes.
        let mut stdin = child.stdin.take().ok_or(GitError::NotFound)?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or(GitError::NotFound)?);
        let failed = |detail: String| GitError::Failed {
            cmd: "cat-file --batch".to_owned(),
            stderr: detail,
        };

        let mut blobs = Vec::with_capacity(oids.len());
        for oid in oids {
            stdin.write_all(format!("{oid}\n").as_bytes())?;
            stdin.flush()?;
            // Response header: `<oid> <type> <size>` or `<oid> missing`.
            let mut header = String::new();
            stdout.read_line(&mut header)?;
            let size: usize = header
                .split_whitespace()
                .nth(2)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| failed(format!("对象 {oid} 不可读: {}", header.trim())))?;
            let mut content = vec![0u8; size];
            stdout.read_exact(&mut content)?;
            // Each object is followed by a newline separator.
            stdout.read_exact(&mut [0u8; 1])?;
            blobs.push(content);
        }
        drop(stdin);
        child.wait()?;
        Ok(blobs)
    }

    /// List local and remote tracking branches available as merge / rebase targets,
    /// excluding the current branch and symbolic HEAD refs.
    pub fn list_branches(&self) -> Result<Vec<String>, GitError> {
        let current = {
            let out = self.run_ok(&["branch", "--show-current"])?;
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let mut branches = Vec::new();
        for prefix in ["refs/heads", "refs/remotes"] {
            branches.extend(
                self.list_refs(prefix)?
                    .into_iter()
                    .filter(|name| *name != current),
            );
        }
        Ok(branches)
    }

    /// List short branch refs under a prefix, excluding symbolic refs such as origin/HEAD.
    ///
    /// Avoid `branch --format=%(refname:short)`: Git versions abbreviate origin/HEAD
    /// differently (origin/HEAD in older versions, origin in newer ones). Name-based
    /// filtering is unreliable; `%(symref)` identifies symbolic refs precisely.
    fn list_refs(&self, prefix: &str) -> Result<Vec<String>, GitError> {
        let out = self.run_ok(&[
            "for-each-ref",
            prefix,
            "--format=%(refname:lstrip=2)\t%(symref)",
        ])?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let (name, symref) = line.split_once('\t').unwrap_or((line.trim(), ""));
                (!name.is_empty() && symref.is_empty()).then(|| name.to_owned())
            })
            .collect())
    }

    /// List switchable branches: exclude the current local branch, symbolic remote HEAD,
    /// and remote entries with local counterparts (hide origin/foo when foo exists)
    /// to avoid duplicate targets in the picker.
    pub fn list_switch_branches(&self) -> Result<SwitchBranches, GitError> {
        let current = {
            let out = self.run_ok(&["branch", "--show-current"])?;
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let locals: Vec<String> = self
            .list_refs("refs/heads")?
            .into_iter()
            .filter(|name| *name != current)
            .collect();
        // Hide remote branches whose name after the remote prefix exists locally (origin/feat/x -> feat/x).
        let remotes = self
            .list_refs("refs/remotes")?
            .into_iter()
            .filter(|name| {
                let short = name.split_once('/').map_or(name.as_str(), |(_, rest)| rest);
                short != current && !locals.iter().any(|l| l == short)
            })
            .collect();
        Ok(SwitchBranches { locals, remotes })
    }

    /// Whether a full ref exists, such as `refs/heads/foo` / `refs/remotes/origin/foo`.
    pub fn ref_exists(&self, full_ref: &str) -> Result<bool, GitError> {
        Ok(self
            .run(&["show-ref", "--verify", "--quiet", full_ref])?
            .status
            .success())
    }

    /// Recent commits as `--oneline` entries with a short hash first, for the commit picker.
    ///
    /// With `others_only`, list commits outside the current branch for cherry-pick;
    /// otherwise list recent commits on this branch for revert.
    /// Return an empty list when no commits exist, such as in an empty repository.
    pub fn recent_commits(&self, others_only: bool, limit: usize) -> Result<Vec<String>, GitError> {
        let n = format!("-n{limit}");
        let mut args = vec!["log", "--oneline", &n];
        if others_only {
            args.extend(["--all", "--not", "HEAD"]);
        }
        let out = self.run(&args)?;
        if !out.status.success() {
            return Ok(Vec::new());
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// Query repository stats: branch, changes, stashes, commits ahead, and total commits.
    ///
    /// Five independent queries each cost about 12-16 ms to spawn, so scoped threads
    /// run them concurrently to keep menu startup near the slowest query's duration.
    /// Missing upstream and empty repositories yield nonzero exits for `ahead` and
    /// `level`, normalized to None and 0 respectively.
    pub fn vitals(&self) -> Result<RepoVitals, GitError> {
        // Join workers; query closures return Result, so panic handling is defensive.
        fn joined<T>(handle: std::thread::ScopedJoinHandle<'_, T>) -> Result<T, GitError> {
            handle.join().map_err(|_| GitError::Failed {
                cmd: "vitals".to_owned(),
                stderr: "worker thread panicked".to_owned(),
            })
        }
        let (branch, changes, stashes, ahead, level) = std::thread::scope(|s| {
            let branch = s.spawn(|| self.run_ok(&["branch", "--show-current"]));
            let changes = s.spawn(|| self.run_ok(&["status", "--porcelain"]));
            let stashes = s.spawn(|| self.run_ok(&["stash", "list"]));
            let ahead = s.spawn(|| self.run(&["rev-list", "--count", "@{upstream}..HEAD"]));
            let level = s.spawn(|| self.run(&["rev-list", "--count", "HEAD"]));
            (
                joined(branch),
                joined(changes),
                joined(stashes),
                joined(ahead),
                joined(level),
            )
        });

        let branch = {
            let name = String::from_utf8_lossy(&branch??.stdout).trim().to_owned();
            if name.is_empty() {
                "HEAD".to_owned()
            } else {
                name
            }
        };
        let lines = |out: Output| String::from_utf8_lossy(&out.stdout).lines().count();
        let count = |out: Output| -> Option<usize> {
            if !out.status.success() {
                return None;
            }
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        };
        Ok(RepoVitals {
            branch,
            changes: lines(changes??),
            stashes: lines(stashes??),
            ahead: count(ahead??),
            level: count(level??).unwrap_or(0),
        })
    }

    /// Write resolved content to the worktree and run `git add`.
    pub fn stage_resolved(&self, path: &str, content: &[u8]) -> Result<(), GitError> {
        let full = self.top.join(path);
        // Remove an existing symlink first: fs::write follows it and could overwrite
        // its target, which may be outside the repository.
        if std::fs::symlink_metadata(&full).is_ok_and(|meta| meta.file_type().is_symlink()) {
            std::fs::remove_file(&full)?;
        }
        std::fs::write(&full, content)?;
        self.add_path(path)
    }

    /// Take a whole side of a conflicted file (`stage` 2 local / 3 remote), following
    /// git mergetool semantics for objects that cannot be merged or written as text:
    /// - Submodules: update only the index, preserving the checkout and any unpushed commits.
    /// - Other files: use `checkout-index -f` and `git add`, or `git rm` for a deleted side.
    ///   Git replaces the old path without following symlinks and respects core.symlinks.
    pub fn take_side(&self, file: &ConflictedFile, stage: u8) -> Result<(), GitError> {
        let chosen = match stage {
            2 => &file.ours,
            3 => &file.theirs,
            other => {
                return Err(GitError::Failed {
                    cmd: format!("checkout-index --stage={other}"),
                    stderr: "stage must be 2 (ours) or 3 (theirs)".to_owned(),
                });
            }
        };
        let path = file.path.as_str();
        if file.is_submodule() {
            match chosen {
                Some(entry) => {
                    let info = format!("{:o},{},{path}", entry.mode, entry.oid);
                    self.run_ok(&["update-index", "--cacheinfo", &info])?;
                }
                None => {
                    self.run_ok(&["update-index", "--force-remove", "--", path])?;
                }
            }
            return Ok(());
        }
        if chosen.is_none() {
            self.run_ok(&["--literal-pathspecs", "rm", "-q", "--", path])?;
            return Ok(());
        }
        let stage_arg = format!("--stage={stage}");
        self.run_ok(&["checkout-index", "-f", &stage_arg, "--", path])?;
        self.add_path(path)
    }

    /// Run `git add` with a literal path: glob characters such as `*`, `?`, and `[` in
    /// filenames must not stage unrelated matching files into the merge commit.
    fn add_path(&self, path: &str) -> Result<(), GitError> {
        self.run_ok(&["--literal-pathspecs", "add", "--", path])?;
        Ok(())
    }

    /// Continue the current merge / rebase after resolving all conflicts.
    ///
    /// Stream Git and hook output, including colors, directly to the terminal.
    /// Return the exit code because rebase --continue can exit nonzero when the next
    /// commit conflicts; the caller probes conflicts to decide whether it failed.
    pub fn continue_op(&self, state: RepoState) -> Result<ExitStatus, GitError> {
        let op = match state {
            RepoState::Clean => {
                return Err(GitError::Failed {
                    cmd: "--continue".to_owned(),
                    stderr: crate::i18n::tr("git.no_op").to_owned(),
                });
            }
            other => other.op_name(),
        };
        self.run_inherit(&["-c", "core.editor=true", op, "--continue"])
    }

    /// Abort the current merge / rebase.
    pub fn abort_op(&self, state: RepoState) -> Result<(), GitError> {
        let op = match state {
            RepoState::Clean => {
                return Err(GitError::Failed {
                    cmd: "--abort".to_owned(),
                    stderr: crate::i18n::tr("git.no_op").to_owned(),
                });
            }
            other => other.op_name(),
        };
        self.run_ok(&[op, "--abort"]).map(|_| ())
    }
}

/// Parse `git ls-files -u -z` output.
///
/// NUL-separated entries have the form `<mode> <oid> <stage>\t<path>`.
/// Group the one to three stage entries for a path into one [`ConflictedFile`].
fn parse_ls_files_unmerged(text: &str) -> Vec<ConflictedFile> {
    let mut files: Vec<ConflictedFile> = Vec::new();
    for entry in text.split('\0').filter(|e| !e.is_empty()) {
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut fields = meta.split_whitespace();
        let (Some(mode), Some(oid), Some(stage)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(mode) = u32::from_str_radix(mode, 8) else {
            continue;
        };
        let idx = match files.iter().position(|f| f.path == path) {
            Some(i) => i,
            None => {
                files.push(ConflictedFile {
                    path: path.to_owned(),
                    base: None,
                    ours: None,
                    theirs: None,
                });
                files.len() - 1
            }
        };
        let entry = Some(Stage {
            mode,
            oid: oid.to_owned(),
        });
        match stage {
            "1" => files[idx].base = entry,
            "2" => files[idx].ours = entry,
            "3" => files[idx].theirs = entry,
            _ => {}
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stage entry for a regular file.
    fn regular(oid: &str) -> Option<Stage> {
        Some(Stage {
            mode: 0o100644,
            oid: oid.to_owned(),
        })
    }

    #[test]
    fn parses_unmerged_entries_grouped_by_path() {
        let text = "100644 aaaa 1\tsrc/a.rs\x00100644 bbbb 2\tsrc/a.rs\x00100644 cccc 3\tsrc/a.rs\x00100644 dddd 2\tREADME.md\x00100644 eeee 3\tREADME.md\x00";
        let files = parse_ls_files_unmerged(text);
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0],
            ConflictedFile {
                path: "src/a.rs".to_owned(),
                base: regular("aaaa"),
                ours: regular("bbbb"),
                theirs: regular("cccc"),
            }
        );
        assert!(!files[0].is_symlink());
        // Add/add conflict: no stage 1.
        assert!(files[1].base.is_none());
        assert!(files[1].ours.is_some() && files[1].theirs.is_some());
    }

    /// Parse modes as octal; a symlink in any stage identifies a link conflict.
    /// merge-ort splits symlink/file type changes into two paths, possibly leaving one link stage.
    #[test]
    fn detects_symlink_stages() {
        let text = "120000 aaaa 1\tlink\x00120000 bbbb 2\tlink\x00120000 cccc 3\tlink\x00120000 dddd 2\tx\x00";
        let files = parse_ls_files_unmerged(text);
        assert_eq!(files[0].ours.as_ref().map(|s| s.mode), Some(MODE_SYMLINK));
        assert!(files[0].is_symlink());
        assert!(files[1].is_symlink() && files[1].theirs.is_none());
        // Skip entries with invalid modes.
        assert!(parse_ls_files_unmerged("12x000 aaaa 2\tbad\x00").is_empty());
    }

    /// A gitlink in any stage identifies a submodule conflict.
    #[test]
    fn detects_submodule_stages() {
        let text = "160000 aaaa 1\tsub\x00160000 bbbb 2\tsub\x00160000 cccc 3\tsub\x00";
        let files = parse_ls_files_unmerged(text);
        assert!(files[0].is_submodule() && !files[0].is_symlink());
        assert_eq!(files[0].theirs.as_ref().map(|s| s.mode), Some(MODE_GITLINK));
    }

    #[test]
    fn parses_empty_output() {
        assert!(parse_ls_files_unmerged("").is_empty());
    }

    #[test]
    fn tolerates_tab_in_path() {
        // With -z, paths are unescaped; everything after the first tab is the path.
        let text = "100644 aaaa 2\ta\tb.txt\0";
        let files = parse_ls_files_unmerged(text);
        assert_eq!(files[0].path, "a\tb.txt");
    }
}
