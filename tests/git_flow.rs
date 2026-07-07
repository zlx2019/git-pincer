//! Integration tests for the git wrapper: build real git repositories in a
//! temporary directory and verify the whole conflict-handling flow.
//!
//! Requires git on the machine (CI runners have it). Every test uses its own
//! temporary repository, and repo-local configuration isolates the tests from
//! the user's global settings (signing, hooks, etc.).

// clippy.toml 的 allow-unwrap-in-tests 只覆盖 #[test] 函数体,
// 集成测试的辅助函数不在其列,这里在测试 crate 层面显式放行
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::Command;

use git_peace::git::{Git, GitError, RepoState};

/// 临时 git 仓库守卫:Drop 时清理目录。
///
struct TempRepo {
    dir: PathBuf,
}

/// 构造干净的 git 命令:清掉外层 git(如 pre-commit 钩子里跑测试时)
/// 注入的宿主仓库环境变量,避免嵌套调用被劫持到错误的仓库。
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
    /// 新建并初始化仓库(局部配置身份信息、关闭签名)。
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("git-peace-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Self { dir };
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "tester"]);
        repo.git(&["config", "user.email", "tester@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    /// 在仓库目录执行 git 命令并断言成功,返回 stdout。
    fn git(&self, args: &[&str]) -> String {
        let out = clean_git(&self.dir).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} 失败: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// 执行允许失败的 git 命令,返回是否成功。
    fn git_allow_fail(&self, args: &[&str]) -> bool {
        clean_git(&self.dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    /// 写入仓库内文件。
    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.dir.join(name), content).unwrap();
    }

    /// add 全部并提交。
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

/// 制造一个 main / feature 同行修改的 merge 冲突现场。
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
        "merge 应产生冲突"
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
    assert!(files[0].has_base && files[0].has_ours && files[0].has_theirs);
}

#[test]
fn reads_three_stages() {
    let repo = conflicted_repo("stages");
    let git = Git::discover(&repo.dir, false).unwrap();

    let base = String::from_utf8(git.read_stage("config.toml", 1).unwrap()).unwrap();
    let ours = String::from_utf8(git.read_stage("config.toml", 2).unwrap()).unwrap();
    let theirs = String::from_utf8(git.read_stage("config.toml", 3).unwrap()).unwrap();
    assert!(base.contains("127.0.0.1"));
    assert!(ours.contains("192.168.1.1")); // 当前分支 main = ours
    assert!(theirs.contains("10.0.0.1")); // 被合并的 feature = theirs
}

#[test]
fn resolve_and_continue_completes_merge() {
    let repo = conflicted_repo("resolve");
    let git = Git::discover(&repo.dir, false).unwrap();

    git.stage_resolved("config.toml", b"host = \"merged\"\nport = 8080\n")
        .unwrap();
    assert!(git.conflicted_files().unwrap().is_empty());

    let out = git.continue_op(RepoState::Merging).unwrap();
    assert!(
        out.status.success(),
        "merge --continue 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(git.state().unwrap(), RepoState::Clean);

    // 合并提交已生成,工作区内容为解决后的版本
    assert!(repo.git(&["log", "--oneline"]).lines().count() >= 4);
    let content = std::fs::read_to_string(repo.dir.join("config.toml")).unwrap();
    assert!(content.contains("merged"));
}

#[test]
fn abort_restores_clean_state() {
    let repo = conflicted_repo("abort");
    let git = Git::discover(&repo.dir, false).unwrap();
    git.abort_op(RepoState::Merging).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Clean);
    // 工作区恢复为 main 侧内容
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
        "rebase 应产生冲突"
    );

    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Rebasing);
    assert_eq!(git.conflicted_files().unwrap().len(), 1);
    git.abort_op(RepoState::Rebasing).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Clean);
}

/// pre-commit 钩子拒绝提交时,--continue 非零退出、钩子输出可被捕获
/// (git 会把钩子输出转到 stderr),仓库保持 merging 状态,
/// 修复后可重试且已解决的冲突不丢。
#[test]
#[cfg(unix)]
fn continue_failure_keeps_merging_state_and_surfaces_hook_output() {
    use std::os::unix::fs::PermissionsExt;

    let repo = conflicted_repo("hookfail");
    let hook = repo.dir.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho hook-rejected\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let git = Git::discover(&repo.dir, false).unwrap();
    git.stage_resolved("config.toml", b"host = \"merged\"\n")
        .unwrap();

    let out = git.continue_op(RepoState::Merging).unwrap();
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("hook-rejected"));
    assert_eq!(git.state().unwrap(), RepoState::Merging);
    assert!(git.conflicted_files().unwrap().is_empty());
}

#[test]
fn discover_fails_outside_repo() {
    let dir = std::env::temp_dir().join(format!("git-peace-test-{}-plain", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let result = Git::discover(&dir, false);
    assert!(matches!(result, Err(GitError::NotARepo)));
    let _ = std::fs::remove_dir_all(&dir);
}
