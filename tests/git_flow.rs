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

use git_pincer::app::{FileEntry, PickKind, Side};
use git_pincer::commands::resolve::{build_session, persist};
use git_pincer::git::{Git, GitError, RepoState};

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
            std::env::temp_dir().join(format!("git-pincer-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Self { dir };
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "tester"]);
        repo.git(&["config", "user.email", "tester@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        // Windows runner 默认 autocrlf=true 会改写行尾,固定关闭保证跨平台确定性
        repo.git(&["config", "core.autocrlf", "false"]);
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
    assert!(ours.contains("192.168.1.1")); // 当前分支 main = ours
    assert!(theirs.contains("10.0.0.1")); // 被合并的 feature = theirs

    // cat-file --batch 批量读取与逐 stage git show 的结果一致
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
    assert!(status.success(), "merge --continue 失败(输出见测试日志)");
    assert_eq!(git.state().unwrap(), RepoState::Clean);

    // 合并提交已生成,工作区内容为解决后的版本
    assert!(repo.git(&["log", "--oneline"]).lines().count() >= 4);
    let content = std::fs::read_to_string(repo.dir.join("config.toml")).unwrap();
    assert!(content.contains("merged"));
}

/// 冲突文件名含 glob 字符时按字面暂存:`[ab].txt` 作 pathspec 会匹配
/// a.txt / b.txt,不能把与合并无关的本地改动和未跟踪文件一并暂存
/// (方括号在 Windows 文件名中同样合法)
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
    // 与本次合并无关的本地改动与未跟踪文件
    repo.write("a.txt", "local wip\n");
    repo.write("b.txt", "untracked\n");
    assert!(!repo.git_allow_fail(&["merge", "--no-edit", "feature"]));

    let git = Git::discover(&repo.dir, false).unwrap();
    git.stage_resolved("[ab].txt", b"resolved\n").unwrap();
    assert!(git.conflicted_files().unwrap().is_empty());
    let status = repo.git(&["status", "--porcelain"]);
    assert!(
        status.lines().any(|l| l == " M a.txt"),
        "a.txt 不应被暂存:\n{status}"
    );
    assert!(
        status.lines().any(|l| l == "?? b.txt"),
        "b.txt 应保持未跟踪:\n{status}"
    );
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

/// pre-commit 钩子拒绝提交时,--continue 非零退出(钩子输出经透传模式
/// 直接流向终端),仓库保持 merging 状态,修复后可重试且已解决的冲突不丢。
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
        "cherry-pick 应产生冲突"
    );

    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::CherryPicking);
    assert_eq!(git.conflicted_files().unwrap().len(), 1);

    // 解决后 --continue 完成摘取,生成新提交
    git.stage_resolved("a.txt", b"resolved\n").unwrap();
    let status = git.continue_op(RepoState::CherryPicking).unwrap();
    assert!(status.success(), "cherry-pick --continue 失败");
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
    // 撤销 c2(v1→v2)需套用反向 diff,但内容已是 v3 → 冲突
    assert!(
        !repo.git_allow_fail(&["revert", "--no-edit", &target]),
        "revert 应产生冲突"
    );

    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Reverting);
    assert_eq!(git.conflicted_files().unwrap().len(), 1);

    git.abort_op(RepoState::Reverting).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Clean);
    let content = std::fs::read_to_string(repo.dir.join("a.txt")).unwrap();
    assert_eq!(content, "v3\n");
}

/// git am 与 rebase 共用 rebase-apply 目录,必须按 applying 标记区分,
/// 否则会错误地用 rebase --continue 收尾。
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
    // 补丁写在 divergent 提交之后,保持未跟踪状态
    repo.write("fix.patch", &patch);
    assert!(
        !repo.git_allow_fail(&["am", "-3", "fix.patch"]),
        "am 应产生冲突"
    );

    let git = Git::discover(&repo.dir, false).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Am);
    git.abort_op(RepoState::Am).unwrap();
    assert_eq!(git.state().unwrap(), RepoState::Clean);
}

/// 菜单数据源:分支列表排除当前分支,提交列表按是否在 HEAD 上过滤
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
    assert!(!branches.contains(&"main".to_owned()), "当前分支应被排除");

    let own = git.recent_commits(false, 10).unwrap();
    assert!(own.iter().any(|l| l.contains("first")));
    let others = git.recent_commits(true, 10).unwrap();
    assert!(others.iter().any(|l| l.contains("feature only")));
    assert!(
        !others.iter().any(|l| l.contains("first")),
        "HEAD 可达的提交应被排除"
    );
}

/// 克隆 origin 仓库,得到带远程跟踪分支的工作仓库。
fn cloned_repo(origin: &TempRepo, name: &str) -> TempRepo {
    let dir = std::env::temp_dir().join(format!("git-pincer-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // autocrlf 必须在克隆时就关闭(-c 写入新仓库配置并在检出前生效):
    // Windows runner 默认 autocrlf=true,若克隆后再改配置,已检出的 CRLF
    // 工作区会被视为本地改动,后续 switch 全部被 git 拒绝
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
        "clone 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let repo = TempRepo { dir };
    repo.git(&["config", "user.name", "tester"]);
    repo.git(&["config", "user.email", "tester@example.com"]);
    repo.git(&["config", "commit.gpgsign", "false"]);
    repo
}

/// 造一个 origin(main / feature 内容不同)并克隆出工作仓库。
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

/// switch 分支清单:当前分支与其远程对应项被排除,本地已有的远程项被去重
#[test]
fn switch_branch_listing_dedupes_remote_counterparts() {
    let (_origin, repo) = repo_with_remote("switch-list");
    let git = Git::discover(&repo.dir, false).unwrap();

    // 克隆初始:本地只有 main(当前,被排除);origin/main 因对应当前分支隐藏
    let branches = git.list_switch_branches().unwrap();
    assert!(branches.locals.is_empty());
    assert_eq!(branches.remotes, vec!["origin/feature".to_owned()]);

    // 建立本地 feature 后,origin/feature 应从远程列表消失
    repo.git(&["branch", "feature", "origin/feature"]);
    let branches = git.list_switch_branches().unwrap();
    assert_eq!(branches.locals, vec!["feature".to_owned()]);
    assert!(branches.remotes.is_empty());
}

/// switch 命令组装与真实切换:本地直切、远程 --track 建跟踪分支、脏树被拒
#[test]
fn switch_resolves_targets_and_switches() {
    let (_origin, repo) = repo_with_remote("switch-flow");
    let git = Git::discover(&repo.dir, false).unwrap();
    let current = |r: &TempRepo| r.git(&["branch", "--show-current"]).trim().to_owned();

    // 远程目标 → --track;未知目标原样交给 git
    let cmd = git_pincer::commands::switch::switch_cmd(&git, "origin/feature").unwrap();
    assert_eq!(cmd, ["switch", "--track", "origin/feature"]);
    let cmd = git_pincer::commands::switch::switch_cmd(&git, "nope").unwrap();
    assert_eq!(cmd, ["switch", "nope"]);

    // 真实切换到远程分支:本地跟踪分支建立,内容与上游一致
    repo.git(&["switch", "--track", "origin/feature"]);
    assert_eq!(current(&repo), "feature");
    let upstream = repo.git(&["rev-parse", "--abbrev-ref", "feature@{upstream}"]);
    assert_eq!(upstream.trim(), "origin/feature");

    // 本地目标 → 直切
    let cmd = git_pincer::commands::switch::switch_cmd(&git, "main").unwrap();
    assert_eq!(cmd, ["switch", "main"]);
    repo.git(&["switch", "main"]);
    assert_eq!(current(&repo), "main");

    // 脏工作区会被覆盖时 git 拒绝切换,停留在原分支(错误原样透传给上层)
    repo.write("a.txt", "dirty\n");
    assert!(!repo.git_allow_fail(&["switch", "feature"]), "脏树应被拒绝");
    assert_eq!(current(&repo), "main");
}

/// 菜单模式执行:非冲突失败返回 Failed(弹框数据源)而非直接报错
#[test]
fn launch_captured_reports_failure_without_bailing() {
    let repo = TempRepo::new("trylaunch");
    repo.write("a.txt", "one\n");
    repo.commit_all("init");

    let git = git_pincer::git::Git::discover(&repo.dir, false).unwrap();
    let outcome = git_pincer::commands::run::launch_captured(&git, &["pull"]).unwrap();
    match outcome {
        git_pincer::commands::run::LaunchOutcome::Failed(reason) => assert!(!reason.is_empty()),
        other => panic!("无远程仓库的 pull 应返回 Failed,实际为 {other:?}"),
    }
}

/// 按真实编排路径解决首个冲突文件(须已降级为二选一):构建会话 → 选边 → 落盘,
/// 返回会话中的条目供断言。
fn resolve_first_by_pick(git: &Git, side: Side) -> FileEntry {
    let files = git.conflicted_files().unwrap();
    let mut session = build_session(git, &files, true).unwrap();
    let mut entry = session.files.remove(0);
    let FileEntry::Pick { choice, .. } = &mut entry else {
        panic!("应降级为二选一条目,实际为 {entry:?}");
    };
    *choice = Some(side);
    persist(git, &files, &entry).unwrap();
    entry
}

/// 二进制冲突整文件二选一:取远端后写入远端内容并完成暂存
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

/// 把工作区的符号链接改指向 target。
#[cfg(unix)]
fn relink(repo: &TempRepo, name: &str, target: &str) {
    let link = repo.dir.join(name);
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// 符号链接冲突:取远端后链接改指向远端目标,本地链接原先指向的文件
/// 不被写穿,暂存的仍是符号链接
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
    assert_eq!(b, "ours-target\n", "链接目标被写穿");
    assert!(repo.git(&["ls-files", "-s", "link"]).starts_with("120000"));
    assert!(git.conflicted_files().unwrap().is_empty());
    assert!(git.continue_op(RepoState::Merging).unwrap().success());
}

/// 符号链接修改 / 删除冲突取删除侧:删除链接本身,链接目标保持不变
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

/// 工作区路径是符号链接时,stage_resolved 替换链接本身而非写进链接目标
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
    assert_eq!(precious, "precious\n", "链接目标被写穿");
    let config = repo.dir.join("config.toml");
    assert!(std::fs::symlink_metadata(&config).unwrap().is_file());
    assert_eq!(
        std::fs::read_to_string(config).unwrap(),
        "host = \"merged\"\n"
    );
}

/// 伪造的子模块提交 id(gitlink 只记录 id,无需真实子仓库)。
const SUB_BASE: &str = "1111111111111111111111111111111111111111";
const SUB_OURS: &str = "2222222222222222222222222222222222222222";
const SUB_THEIRS: &str = "3333333333333333333333333333333333333333";

/// 把 index 中的子模块 sub 指向给定提交(不用 add -A,避免把空目录当作删除)。
fn set_gitlink(repo: &TempRepo, oid: &str) {
    let info = format!("160000,{oid},sub");
    repo.git(&["update-index", "--add", "--cacheinfo", &info]);
}

/// 子模块指针分叉 + 普通文本冲突同时出现的 merge 现场。
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

/// 子模块冲突不阻塞同轮其他文件:会话照常构建,子模块按提交整条二选一,
/// 只暂存所选提交,子模块工作区原样保留
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
        "同轮的文本冲突应照常逐块合并"
    );
    let FileEntry::Pick {
        kind,
        ours,
        theirs,
        choice,
        ..
    } = &mut session.files[0]
    else {
        panic!("子模块冲突应降级为二选一");
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

/// 子模块修改 / 删除冲突取删除侧:只从 index 移除,子模块工作区原样保留
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
