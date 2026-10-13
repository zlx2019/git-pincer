//! Integration tests for the git wrapper: build real git repositories in a
//! temporary directory and verify the whole conflict-handling flow.
//!
//! Requires git on the machine (CI runners have it). Every test uses its own
//! temporary repository, and repo-local configuration isolates the tests from
//! the user's global settings (signing, hooks, etc.).

// clippy.toml's allow-*-in-tests switches only cover `#[test]` function bodies;
// helper functions of an integration test are not covered, so allow them for the whole test crate
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

use git_pincer::app::{FileEntry, PickKind, Side};
use git_pincer::commands::resolve::{build_session, persist};
use git_pincer::git::{Git, GitError, RepoState};

/// Temporary Git repository guard that removes its directory on Drop.
///
struct TempRepo {
    dir: PathBuf,
}

/// Build a clean Git command, removing host repository variables injected by
/// outer Git processes such as pre-commit hooks so nested calls use the correct repository.
fn clean_git(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

impl TempRepo {
    /// Initialize a repository with local identity configuration and signing disabled.
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("git-pincer-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Self { dir };
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "tester"]);
        repo.git(&["config", "user.email", "tester@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        // Disable autocrlf, enabled by default on Windows runners, for deterministic line endings.
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }

    /// Run Git in the repository, assert success, and return stdout.
    fn git(&self, args: &[&str]) -> String {
        let out = clean_git(&self.dir).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Run a Git command that may fail and return whether it succeeded.
    fn git_allow_fail(&self, args: &[&str]) -> bool {
        clean_git(&self.dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    /// Write a file in the repository.
    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.dir.join(name), content).unwrap();
    }

    /// Stage all files and commit.
    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", message]);
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Create a merge conflict where main and feature modify the same line.
fn conflicted_repo(name: &str) -> TempRepo {
    let repo = TempRepo::new(name);
    repo.write("config.toml", "host = \"127.0.0.1\"\nport = 8080\n");
    repo.commit_all("init");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("config.toml", "host = \"10.0.0.1\"\nport = 8080\n");
    repo.commit_all("feature change");
    repo.git(&["checkout", "main"]);
    repo.write("config.toml", "host = \"192.168.1.1\"\nport = 8080\n");
    repo.commit_all("main change");
    assert!(
        !repo.git_allow_fail(&["merge", "--no-edit", "feature"]),
        "merge must produce a conflict"
    );
    repo
}

#[test]
fn detects_merge_state_and_conflicts() {
    let repo = conflicted_repo("detect");
    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Merging);

    let files = git.conflicted_files().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "config.toml");
    assert!(files[0].base.is_some() && files[0].ours.is_some() && files[0].theirs.is_some());
}

#[test]
fn reads_three_stages() {
    let repo = conflicted_repo("stages");
    let git = Git::discover(&repo.dir, false).unwrap();

    let base = String::from_utf8(git.read_stage("config.toml", 1).unwrap()).unwrap();
    let ours = String::from_utf8(git.read_stage("config.toml", 2).unwrap()).unwrap();
    let theirs = String::from_utf8(git.read_stage("config.toml", 3).unwrap()).unwrap();
    assert!(base.contains("127.0.0.1"));
    assert!(ours.contains("192.168.1.1")); // Current branch main is ours.
    assert!(theirs.contains("10.0.0.1")); // Merged branch feature is theirs.

    // Batched cat-file --batch reads match individual stage reads through git show.
    let files = git.conflicted_files().unwrap();
    let f = &files[0];
    let oids: Vec<&str> = [&f.base, &f.ours, &f.theirs]
        .into_iter()
        .flatten()
        .map(|stage| stage.oid.as_str())
        .collect();
    let blobs = git.read_blobs(&oids).unwrap();
    assert_eq!(blobs.len(), 3);
    assert_eq!(String::from_utf8(blobs[0].clone()).unwrap(), base);
    assert_eq!(String::from_utf8(blobs[1].clone()).unwrap(), ours);
    assert_eq!(String::from_utf8(blobs[2].clone()).unwrap(), theirs);
}

#[test]
fn resolve_and_continue_completes_merge() {
    let repo = conflicted_repo("resolve");
    let git = Git::discover(&repo.dir, false).unwrap();

    git.stage_resolved("config.toml", b"host = \"merged\"\nport = 8080\n")
        .unwrap();
    assert!(git.conflicted_files().unwrap().is_empty());

    let status = git.continue_op(RepoState::Merging).unwrap();
    assert!(
        status.success(),
        "merge --continue failed (see test logs for output)"
    );
    assert_eq!(git.state().unwrap(), RepoState::Clean);

    // The merge commit exists and the worktree contains the resolved version.
    assert!(repo.git(&["log", "--oneline"]).lines().count() >= 4);
    let content = std::fs::read_to_string(repo.dir.join("config.toml")).unwrap();
    assert!(content.contains("merged"));
}

/// Stage conflicted filenames literally: interpreting `[ab].txt` as a pathspec
/// would match a.txt / b.txt and stage unrelated local changes or untracked files.
/// Square brackets are also valid in Windows filenames.
#[test]
fn stage_resolved_matches_path_literally() {
    let repo = TempRepo::new("literal");
    repo.write("[ab].txt", "base\n");
    repo.write("a.txt", "a\n");
    repo.commit_all("init");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("[ab].txt", "feature\n");
    repo.commit_all("feature change");
    repo.git(&["checkout", "main"]);
    repo.write("[ab].txt", "main\n");
    repo.commit_all("main change");
    // Local changes and untracked files unrelated to this merge.
    repo.write("a.txt", "local wip\n");
    repo.write("b.txt", "untracked\n");
    assert!(!repo.git_allow_fail(&["merge", "--no-edit", "feature"]));

    let git = Git::discover(&repo.dir, false).unwrap();
    git.stage_resolved("[ab].txt", b"resolved\n").unwrap();
    assert!(git.conflicted_files().unwrap().is_empty());
    let status = repo.git(&["status", "--porcelain"]);
    assert!(
        status.lines().any(|l| l == " M a.txt"),
        "a.txt must not be staged:\n{status}"
    );
    assert!(
        status.lines().any(|l| l == "?? b.txt"),
        "b.txt must remain untracked:\n{status}"
    );
}

#[test]
fn abort_restores_clean_state() {
    let repo = conflicted_repo("abort");
    let git = Git::discover(&repo.dir, false).unwrap();
    git.abort_op(RepoState::Merging).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Clean);
    // The worktree is restored to main's content.
    let content = std::fs::read_to_string(repo.dir.join("config.toml")).unwrap();
    assert!(content.contains("192.168.1.1"));
}

#[test]
fn rebase_conflict_is_detected_and_abortable() {
    let repo = TempRepo::new("rebase");
    repo.write("a.txt", "one\n");
    repo.commit_all("init");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("a.txt", "feature\n");
    repo.commit_all("feature change");
    repo.git(&["checkout", "main"]);
    repo.write("a.txt", "main\n");
    repo.commit_all("main change");
    repo.git(&["checkout", "feature"]);
    assert!(
        !repo.git_allow_fail(&["rebase", "main"]),
        "rebase must produce a conflict"
    );

    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Rebasing);
    assert_eq!(git.conflicted_files().unwrap().len(), 1);
    git.abort_op(RepoState::Rebasing).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Clean);
}

/// A rejecting pre-commit hook makes --continue exit nonzero with streamed output.
/// The repository remains merging, allowing a retry without losing resolved conflicts.
#[test]
#[cfg(unix)]
fn continue_failure_keeps_merging_state() {
    use std::os::unix::fs::PermissionsExt;

    let repo = conflicted_repo("hookfail");
    let hook = repo.dir.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho hook-rejected\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let git = Git::discover(&repo.dir, false).unwrap();
    git.stage_resolved("config.toml", b"host = \"merged\"\n")
        .unwrap();

    let status = git.continue_op(RepoState::Merging).unwrap();
    assert!(!status.success());
    assert_eq!(git.state().unwrap(), RepoState::Merging);
    assert!(git.conflicted_files().unwrap().is_empty());
}

#[test]
fn discover_fails_outside_repo() {
    let dir = std::env::temp_dir().join(format!("git-pincer-test-{}-plain", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let result = Git::discover(&dir, false);
    assert!(matches!(result, Err(GitError::NotARepo)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cherry_pick_conflict_detected_and_continues() {
    let repo = TempRepo::new("cherrypick");
    repo.write("a.txt", "base\n");
    repo.commit_all("init");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("a.txt", "feature\n");
    repo.commit_all("feature change");
    let commit = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    repo.git(&["checkout", "main"]);
    repo.write("a.txt", "main\n");
    repo.commit_all("main change");
    assert!(
        !repo.git_allow_fail(&["cherry-pick", &commit]),
        "cherry-pick must produce a conflict"
    );

    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::CherryPicking);
    assert_eq!(git.conflicted_files().unwrap().len(), 1);

    // After resolution, --continue finishes the cherry-pick and creates a commit.
    git.stage_resolved("a.txt", b"resolved\n").unwrap();
    let status = git.continue_op(RepoState::CherryPicking).unwrap();
    assert!(status.success(), "cherry-pick --continue failed");
    assert_eq!(git.state().unwrap(), RepoState::Clean);
    assert!(repo.git(&["log", "--oneline"]).contains("feature change"));
}

#[test]
fn revert_conflict_detected_and_abortable() {
    let repo = TempRepo::new("revert");
    repo.write("a.txt", "v1\n");
    repo.commit_all("c1");
    repo.write("a.txt", "v2\n");
    repo.commit_all("c2");
    let target = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    repo.write("a.txt", "v3\n");
    repo.commit_all("c3");
    // Reverting c2 (v1 -> v2) applies its inverse diff to v3, causing a conflict.
    assert!(
        !repo.git_allow_fail(&["revert", "--no-edit", &target]),
        "revert must produce a conflict"
    );

    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Reverting);
    assert_eq!(git.conflicted_files().unwrap().len(), 1);

    git.abort_op(RepoState::Reverting).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Clean);
    let content = std::fs::read_to_string(repo.dir.join("a.txt")).unwrap();
    assert_eq!(content, "v3\n");
}

/// git am and rebase share rebase-apply; distinguish them using the applying marker
/// to avoid incorrectly finishing am with rebase --continue.
#[test]
fn am_state_detected_distinct_from_rebase() {
    let repo = TempRepo::new("am");
    repo.write("a.txt", "one\n");
    repo.commit_all("c1");
    repo.write("a.txt", "two\n");
    repo.commit_all("c2");
    let patch = repo.git(&["format-patch", "-1", "HEAD", "--stdout"]);
    repo.git(&["reset", "--hard", "HEAD~1"]);
    repo.write("a.txt", "three\n");
    repo.commit_all("divergent");
    // Write the patch after the divergent commit so it remains untracked.
    repo.write("fix.patch", &patch);
    assert!(
        !repo.git_allow_fail(&["am", "-3", "fix.patch"]),
        "am must produce a conflict"
    );

    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Am);
    git.abort_op(RepoState::Am).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Clean);
}

/// Menu data: exclude the current branch and filter commits by reachability from HEAD.
#[test]
fn lists_branches_and_recent_commits() {
    let repo = TempRepo::new("listing");
    repo.write("a.txt", "one\n");
    repo.commit_all("first");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("a.txt", "two\n");
    repo.commit_all("feature only");
    repo.git(&["checkout", "main"]);

    let git = Git::discover(&repo.dir, false).unwrap();
    let branches = git.list_branches().unwrap();
    assert!(branches.contains(&"feature".to_owned()));
    assert!(
        !branches.contains(&"main".to_owned()),
        "current branch must be excluded"
    );

    let own = git.recent_commits(false, 10).unwrap();
    assert!(own.iter().any(|l| l.contains("first")));
    let others = git.recent_commits(true, 10).unwrap();
    assert!(others.iter().any(|l| l.contains("feature only")));
    assert!(
        !others.iter().any(|l| l.contains("first")),
        "commits reachable from HEAD must be excluded"
    );
}

/// Clone origin into a working repository with remote tracking branches.
fn cloned_repo(origin: &TempRepo, name: &str) -> TempRepo {
    let dir = std::env::temp_dir().join(format!("git-pincer-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Disable autocrlf during clone: -c writes configuration before checkout.
    // Windows runners default to autocrlf=true; changing it after checkout would
    // make CRLF files appear modified and cause subsequent switches to fail.
    let out = clean_git(&std::env::temp_dir())
        .args([
            "clone",
            "-c",
            "core.autocrlf=false",
            origin.dir.to_str().unwrap(),
            dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let repo = TempRepo { dir };
    repo.git(&["config", "user.name", "tester"]);
    repo.git(&["config", "user.email", "tester@example.com"]);
    repo.git(&["config", "commit.gpgsign", "false"]);
    repo
}

/// Create origin with different main / feature content and clone a working repository.
fn repo_with_remote(name: &str) -> (TempRepo, TempRepo) {
    let origin = TempRepo::new(&format!("{name}-origin"));
    origin.write("a.txt", "main\n");
    origin.commit_all("init");
    origin.git(&["checkout", "-b", "feature"]);
    origin.write("a.txt", "feature\n");
    origin.commit_all("feature change");
    origin.git(&["checkout", "main"]);
    let clone = cloned_repo(&origin, name);
    (origin, clone)
}

/// Switch branch lists exclude the current branch and remote entries with local counterparts.
#[test]
fn switch_branch_listing_dedupes_remote_counterparts() {
    let (_origin, repo) = repo_with_remote("switch-list");
    let git = Git::discover(&repo.dir, false).unwrap();

    // Initially only main exists locally; it and its remote counterpart origin/main are hidden.
    let branches = git.list_switch_branches().unwrap();
    assert!(branches.locals.is_empty());
    assert_eq!(branches.remotes, vec!["origin/feature".to_owned()]);

    // Creating local feature removes origin/feature from the remote list.
    repo.git(&["branch", "feature", "origin/feature"]);
    let branches = git.list_switch_branches().unwrap();
    assert_eq!(branches.locals, vec!["feature".to_owned()]);
    assert!(branches.remotes.is_empty());
}

/// Build and run switch commands: direct local switches, remote tracking branches, and dirty-tree rejection.
#[test]
fn switch_resolves_targets_and_switches() {
    let (_origin, repo) = repo_with_remote("switch-flow");
    let git = Git::discover(&repo.dir, false).unwrap();
    let current = |r: &TempRepo| r.git(&["branch", "--show-current"]).trim().to_owned();

    // Remote targets use --track; unknown targets are passed unchanged to Git.
    let cmd = git_pincer::commands::switch::switch_cmd(&git, "origin/feature").unwrap();
    assert_eq!(cmd, ["switch", "--track", "origin/feature"]);
    let cmd = git_pincer::commands::switch::switch_cmd(&git, "nope").unwrap();
    assert_eq!(cmd, ["switch", "nope"]);

    // Switching to a remote target creates a local tracking branch with upstream content.
    repo.git(&["switch", "--track", "origin/feature"]);
    assert_eq!(current(&repo), "feature");
    let upstream = repo.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"]);
    assert_eq!(upstream.trim(), "origin/feature");

    // Switch directly to local targets.
    let cmd = git_pincer::commands::switch::switch_cmd(&git, "main").unwrap();
    assert_eq!(cmd, ["switch", "main"]);
    repo.git(&["switch", "main"]);
    assert_eq!(current(&repo), "main");

    // Git refuses switches that overwrite local changes, preserving the branch and forwarding the error.
    repo.write("a.txt", "dirty\n");
    assert!(
        !repo.git_allow_fail(&["switch", "feature"]),
        "dirty worktree must be rejected"
    );
    assert_eq!(current(&repo), "main");
}

/// Menu execution returns non-conflict failures as Failed for dialog display.
#[test]
fn launch_captured_reports_failure_without_bailing() {
    let repo = TempRepo::new("trylaunch");
    repo.write("a.txt", "one\n");
    repo.commit_all("init");

    let git = git_pincer::git::Git::discover(&repo.dir, false).unwrap();
    let outcome = git_pincer::commands::run::launch_captured(&git, &["pull"]).unwrap();
    match outcome {
        git_pincer::commands::run::LaunchOutcome::Failed(reason) => assert!(!reason.is_empty()),
        other => panic!("pull without a remote must return Failed, got {other:?}"),
    }
}

/// Resolve the first conflict through the real whole-file-pick flow: build a session,
/// choose a side, persist it, and return the entry for assertions.
fn resolve_first_by_pick(git: &Git, side: Side) -> FileEntry {
    let files = git.conflicted_files().unwrap();
    let mut session = build_session(git, &files, true).unwrap();
    let mut entry = session.files.remove(0);
    let FileEntry::Pick { choice, .. } = &mut entry else {
        panic!("expected a whole-file pick entry, got {entry:?}");
    };
    *choice = Some(side);
    persist(git, &files, &entry).unwrap();
    entry
}

/// Picking the remote side of a binary conflict writes and stages the remote content.
#[test]
fn binary_conflict_is_picked_whole() {
    let repo = TempRepo::new("binary");
    repo.write("img.bin", "BIN\0base");
    repo.commit_all("init");
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("img.bin", "BIN\0theirs");
    repo.commit_all("feature change");
    repo.git(&["checkout", "main"]);
    repo.write("img.bin", "BIN\0ours");
    repo.commit_all("main change");
    assert!(!repo.git_allow_fail(&["merge", "--no-edit", "feature"]));

    let git = Git::discover(&repo.dir, false).unwrap();
    let entry = resolve_first_by_pick(&git, Side::Theirs);
    assert!(matches!(
        entry,
        FileEntry::Pick {
            kind: PickKind::File,
            ..
        }
    ));
    assert_eq!(
        std::fs::read(repo.dir.join("img.bin")).unwrap(),
        b"BIN\0theirs"
    );
    assert!(git.conflicted_files().unwrap().is_empty());
}

/// Point a worktree symlink to target.
#[cfg(unix)]
fn relink(repo: &TempRepo, name: &str, target: &str) {
    let link = repo.dir.join(name);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// Picking the remote symlink updates its target without overwriting the original
/// linked file; the staged object remains a symlink.
#[cfg(unix)]
#[test]
fn symlink_conflict_is_picked_without_writing_through() {
    let repo = TempRepo::new("symlink");
    repo.write("b.txt", "ours-target\n");
    repo.write("c.txt", "theirs-target\n");
    relink(&repo, "link", "a.txt");
    repo.commit_all("init");
    repo.git(&["checkout", "-b", "feature"]);
    relink(&repo, "link", "c.txt");
    repo.commit_all("feature relink");
    repo.git(&["checkout", "main"]);
    relink(&repo, "link", "b.txt");
    repo.commit_all("main relink");
    assert!(!repo.git_allow_fail(&["merge", "--no-edit", "feature"]));

    let git = Git::discover(&repo.dir, false).unwrap();
    let entry = resolve_first_by_pick(&git, Side::Theirs);
    let FileEntry::Pick {
        kind, ours, theirs, ..
    } = entry
    else {
        unreachable!()
    };
    assert_eq!(kind, PickKind::Symlink);
    assert_eq!(
        (ours.unwrap(), theirs.unwrap()),
        (b"b.txt".to_vec(), b"c.txt".to_vec())
    );

    let link = std::fs::read_link(repo.dir.join("link")).unwrap();
    assert_eq!(link, PathBuf::from("c.txt"));
    let b = std::fs::read_to_string(repo.dir.join("b.txt")).unwrap();
    assert_eq!(b, "ours-target\n", "symlink target was overwritten");
    assert!(repo.git(&["ls-files", "-s", "link"]).starts_with("120000"));
    assert!(git.conflicted_files().unwrap().is_empty());
    assert!(git.continue_op(RepoState::Merging).unwrap().success());
}

/// Picking a deleted symlink side removes only the link, preserving its target.
#[cfg(unix)]
#[test]
fn symlink_deleted_side_removes_only_the_link() {
    let repo = TempRepo::new("symlink-del");
    repo.write("b.txt", "target\n");
    relink(&repo, "link", "a.txt");
    repo.commit_all("init");
    repo.git(&["checkout", "-b", "feature"]);
    repo.git(&["rm", "-q", "link"]);
    repo.commit_all("feature delete");
    repo.git(&["checkout", "main"]);
    relink(&repo, "link", "b.txt");
    repo.commit_all("main relink");
    assert!(!repo.git_allow_fail(&["merge", "--no-edit", "feature"]));

    let git = Git::discover(&repo.dir, false).unwrap();
    let entry = resolve_first_by_pick(&git, Side::Theirs);
    assert!(matches!(entry, FileEntry::Pick { theirs: None, .. }));

    assert!(std::fs::symlink_metadata(repo.dir.join("link")).is_err());
    let b = std::fs::read_to_string(repo.dir.join("b.txt")).unwrap();
    assert_eq!(b, "target\n");
    assert!(git.conflicted_files().unwrap().is_empty());
    assert!(git.continue_op(RepoState::Merging).unwrap().success());
    assert!(repo.git(&["ls-files", "link"]).is_empty());
}

/// stage_resolved replaces worktree symlinks instead of writing into their targets.
#[cfg(unix)]
#[test]
fn stage_resolved_replaces_symlink_instead_of_following() {
    let repo = conflicted_repo("symlink-guard");
    repo.write("precious.txt", "precious\n");
    relink(&repo, "config.toml", "precious.txt");

    let git = Git::discover(&repo.dir, false).unwrap();
    git.stage_resolved("config.toml", b"host = \"merged\"\n")
        .unwrap();
    let precious = std::fs::read_to_string(repo.dir.join("precious.txt")).unwrap();
    assert_eq!(precious, "precious\n", "symlink target was overwritten");
    let config = repo.dir.join("config.toml");
    assert!(std::fs::symlink_metadata(&config).unwrap().is_file());
    assert_eq!(
        std::fs::read_to_string(config).unwrap(),
        "host = \"merged\"\n"
    );
}

/// Fake submodule commit IDs; gitlinks record only IDs, requiring no actual submodule repository.
const SUB_BASE: &str = "1111111111111111111111111111111111111111";
const SUB_OURS: &str = "2222222222222222222222222222222222222222";
const SUB_THEIRS: &str = "3333333333333333333333333333333333333333";

/// Point sub in the index to a commit without add -A, which would treat the empty directory as deleted.
fn set_gitlink(repo: &TempRepo, oid: &str) {
    let info = format!("160000,{oid},sub");
    repo.git(&["update-index", "--add", "--cacheinfo", &info]);
}

/// Merge with both divergent submodule pointers and a regular text conflict.
fn submodule_conflict_repo(name: &str) -> TempRepo {
    let repo = TempRepo::new(name);
    let commit = |text: &str, sub: &str, message: &str| {
        repo.write("t.txt", text);
        repo.git(&["add", "t.txt"]);
        set_gitlink(&repo, sub);
        repo.git(&["commit", "-q", "-m", message]);
    };
    commit("base\n", SUB_BASE, "init");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    commit("theirs\n", SUB_THEIRS, "feature change");
    repo.git(&["checkout", "-q", "main"]);
    commit("ours\n", SUB_OURS, "main change");
    assert!(!repo.git_allow_fail(&["merge", "--no-edit", "feature"]));
    repo
}

/// Submodule conflicts do not block other files in the session. Pick a recorded commit
/// and stage only that commit, preserving the submodule worktree.
#[test]
fn submodule_conflict_does_not_block_other_files() {
    let repo = submodule_conflict_repo("submodule");
    std::fs::create_dir_all(repo.dir.join("sub")).unwrap();
    repo.write("sub/work.txt", "unpushed\n");

    let git = Git::discover(&repo.dir, false).unwrap();
    let files = git.conflicted_files().unwrap();
    let mut session = build_session(&git, &files, true).unwrap();
    assert!(
        matches!(&session.files[1], FileEntry::Text(m) if m.path == "t.txt"),
        "text conflicts in the same round must still merge by chunk"
    );
    let FileEntry::Pick {
        kind,
        ours,
        theirs,
        choice,
        ..
    } = &mut session.files[0]
    else {
        panic!("submodule conflicts must use whole-file picks");
    };
    assert_eq!(*kind, PickKind::Submodule);
    assert_eq!(ours.as_deref(), Some(SUB_OURS.as_bytes()));
    assert_eq!(theirs.as_deref(), Some(SUB_THEIRS.as_bytes()));
    *choice = Some(Side::Theirs);
    persist(&git, &files, &session.files[0]).unwrap();
    git.stage_resolved("t.txt", b"resolved\n").unwrap();

    assert!(git.conflicted_files().unwrap().is_empty());
    assert!(git.continue_op(RepoState::Merging).unwrap().success());
    assert_eq!(repo.git(&["rev-parse", "HEAD:sub"]).trim(), SUB_THEIRS);
    let work = std::fs::read_to_string(repo.dir.join("sub/work.txt")).unwrap();
    assert_eq!(work, "unpushed\n");
}

/// Picking a deleted submodule side removes only the index entry, preserving its worktree.
#[test]
fn submodule_deleted_side_keeps_checkout() {
    let repo = TempRepo::new("submodule-del");
    repo.write("keep.txt", "keep\n");
    repo.git(&["add", "keep.txt"]);
    set_gitlink(&repo, SUB_BASE);
    repo.git(&["commit", "-q", "-m", "init"]);
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.git(&["rm", "-q", "--cached", "sub"]);
    repo.git(&["commit", "-q", "-m", "feature delete"]);
    repo.git(&["checkout", "-q", "main"]);
    set_gitlink(&repo, SUB_OURS);
    repo.git(&["commit", "-q", "-m", "main move"]);
    assert!(!repo.git_allow_fail(&["merge", "--no-edit", "feature"]));
    std::fs::create_dir_all(repo.dir.join("sub")).unwrap();
    repo.write("sub/work.txt", "unpushed\n");

    let git = Git::discover(&repo.dir, false).unwrap();
    let entry = resolve_first_by_pick(&git, Side::Theirs);
    assert!(matches!(
        entry,
        FileEntry::Pick {
            kind: PickKind::Submodule,
            theirs: None,
            ..
        }
    ));
    assert!(git.conflicted_files().unwrap().is_empty());
    assert!(repo.git(&["ls-files", "sub"]).is_empty());
    let work = std::fs::read_to_string(repo.dir.join("sub/work.txt")).unwrap();
    assert_eq!(work, "unpushed\n");
    assert!(git.continue_op(RepoState::Merging).unwrap().success());
}

/// Create a modify/delete conflict: main modifies name and feature deletes it.
fn modify_delete_repo(name: &str, file: &str, base: &str, ours: &str) -> TempRepo {
    let repo = TempRepo::new(name);
    repo.write(file, base);
    repo.commit_all("init");
    repo.git(&["checkout", "-b", "feature"]);
    repo.git(&["rm", "-q", file]);
    repo.commit_all("feature delete");
    repo.git(&["checkout", "main"]);
    repo.write(file, ours);
    repo.commit_all("main change");
    assert!(!repo.git_allow_fail(&["merge", "--no-edit", "feature"]));
    repo
}

/// Picking the deleted side removes the file rather than leaving an empty file.
/// Both text and binary modify/delete conflicts use whole-file picks.
#[test]
fn modify_delete_conflict_deletes_when_deleted_side_is_picked() {
    for (name, file, base, ours) in [
        ("md-text", "note.txt", "a\nb\nc\n", "a\nB\nc\n"),
        ("md-binary", "img.bin", "BIN\0base", "BIN\0ours"),
    ] {
        let repo = modify_delete_repo(name, file, base, ours);
        let git = Git::discover(&repo.dir, false).unwrap();
        let entry = resolve_first_by_pick(&git, Side::Theirs);
        assert!(matches!(entry, FileEntry::Pick { theirs: None, .. }));

        assert!(!repo.dir.join(file).exists(), "{file} must be deleted");
        assert!(git.conflicted_files().unwrap().is_empty());
        assert!(git.continue_op(RepoState::Merging).unwrap().success());
        assert!(
            repo.git(&["ls-tree", "HEAD", "--", file]).is_empty(),
            "merge commit must no longer contain {file}"
        );
    }
}

/// Picking the modified side of a modify/delete conflict preserves its updated content.
#[test]
fn modify_delete_conflict_keeps_modified_side() {
    let repo = modify_delete_repo("md-keep", "note.txt", "a\nb\nc\n", "a\nB\nc\n");
    let git = Git::discover(&repo.dir, false).unwrap();
    resolve_first_by_pick(&git, Side::Ours);

    let content = std::fs::read_to_string(repo.dir.join("note.txt")).unwrap();
    assert_eq!(content, "a\nB\nc\n");
    assert!(git.conflicted_files().unwrap().is_empty());
    assert!(git.continue_op(RepoState::Merging).unwrap().success());
    assert_eq!(repo.git(&["show", "HEAD:note.txt"]), "a\nB\nc\n");
}
