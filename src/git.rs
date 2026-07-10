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

/// 仓库状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoState {
    /// 无进行中的合并操作
    Clean,
    /// merge 进行中(存在 MERGE_HEAD)
    Merging,
    /// rebase 进行中(存在 rebase-merge,或不带 applying 标记的 rebase-apply)
    Rebasing,
    /// cherry-pick 进行中(存在 CHERRY_PICK_HEAD)
    CherryPicking,
    /// revert 进行中(存在 REVERT_HEAD)
    Reverting,
    /// git am 打补丁进行中(存在 rebase-apply/applying)
    Am,
}

impl RepoState {
    /// 对应的 git 子命令名(用于 `--continue/--abort` 与提示信息)。
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

/// 一个处于冲突状态的文件及其在 index 中的 stage 分布。
///
/// 各 stage 记录 blob oid(缺失该 stage 时为 None),
/// 供 [`Git::read_blobs`] 单进程批量读取内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictedFile {
    /// 相对仓库根目录的路径
    pub path: String,
    /// stage 1(base)的 blob oid;add/add 冲突时缺失
    pub base: Option<String>,
    /// stage 2(ours)的 blob oid;对方删除本方修改时才缺失本方
    pub ours: Option<String>,
    /// stage 3(theirs)的 blob oid
    pub theirs: Option<String>,
}

/// 仓库体征:主菜单 RPG 状态面板展示的数据,打开菜单时查询一次。
#[derive(Debug, Clone)]
pub struct RepoVitals {
    /// 当前分支名(detached HEAD 时为 "HEAD")
    pub branch: String,
    /// 工作区 + 暂存区改动条目数
    pub changes: usize,
    /// stash 条目数
    pub stashes: usize,
    /// 领先上游的提交数;未设置上游分支时为 None
    pub ahead: Option<usize>,
    /// HEAD 可达的提交总数(空仓库为 0)
    pub level: usize,
}

/// git 调用上下文:锚定仓库根目录,verbose 时回显执行的命令。
pub struct Git {
    /// 仓库根目录;所有命令经 `-C` 锚定,路径统一为根相对
    top: PathBuf,
    /// 是否回显执行的 git 命令(-v)
    verbose: bool,
}

/// 将进程启动错误归一化为 GitError。
fn spawn_err(e: std::io::Error) -> GitError {
    if e.kind() == std::io::ErrorKind::NotFound {
        GitError::NotFound
    } else {
        GitError::Io(e)
    }
}

/// 需要从子进程环境中清除的 git 变量。
///
/// 当本工具从 git 钩子里被间接调起时(如 pre-commit 里跑测试),
/// 外层 git 会注入这些指向宿主仓库的路径;嵌套 git 调用若继承它们,
/// 会被劫持到错误的仓库,必须清掉。
const SCRUBBED_GIT_ENV: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
];

/// 构造锚定到指定目录的基础 git 命令:清理宿主 git 环境 + 禁用交互编辑器。
fn base_git(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).env("GIT_EDITOR", "true");
    for var in SCRUBBED_GIT_ENV {
        cmd.env_remove(var);
    }
    cmd
}

impl Git {
    /// 从指定目录探测 git 仓库根并构造上下文。
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

    /// 仓库根目录。
    pub fn top(&self) -> &Path {
        &self.top
    }

    /// 执行 git 命令并捕获输出(不检查退出码,交给调用方判断)。
    pub fn run(&self, args: &[&str]) -> Result<Output, GitError> {
        if self.verbose {
            eprintln!("[git] git {}", args.join(" "));
        }
        base_git(&self.top).args(args).output().map_err(spawn_err)
    }

    /// 执行 git 命令,非零退出码视为错误。
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

    /// 以透传模式执行 git:输出直接接到用户终端,
    /// 用于发起 merge / rebase / pull(保留 git 自身的进度显示)。
    pub fn run_inherit(&self, args: &[&str]) -> Result<ExitStatus, GitError> {
        if self.verbose {
            eprintln!("[git] git {}", args.join(" "));
        }
        base_git(&self.top).args(args).status().map_err(spawn_err)
    }

    /// 探测仓库当前的合并状态。
    pub fn state(&self) -> Result<RepoState, GitError> {
        let out = self.run_ok(&["rev-parse", "--git-dir"])?;
        let raw = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        // --git-dir 可能返回相对路径(相对仓库根)
        let git_dir = {
            let p = PathBuf::from(&raw);
            if p.is_absolute() { p } else { self.top.join(p) }
        };
        // rebase 必须先于 cherry-pick 判定:交互式 rebase 内部逐个重放提交,
        // 冲突时也会留下 CHERRY_PICK_HEAD,但收尾命令是 rebase --continue
        if git_dir.join("rebase-apply").exists() {
            // rebase-apply/applying 是 git am 的标记(git 自身也以此区分两者)
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

    /// 列出所有处于冲突状态的文件。
    pub fn conflicted_files(&self) -> Result<Vec<ConflictedFile>, GitError> {
        let out = self.run_ok(&["ls-files", "-u", "-z"])?;
        Ok(parse_ls_files_unmerged(&String::from_utf8_lossy(
            &out.stdout,
        )))
    }

    /// 读取冲突文件某个 stage 的完整内容(1=base,2=ours,3=theirs)。
    pub fn read_stage(&self, path: &str, stage: u8) -> Result<Vec<u8>, GitError> {
        let spec = format!(":{stage}:{path}");
        Ok(self.run_ok(&["show", &spec])?.stdout)
    }

    /// 用单个 `git cat-file --batch` 进程批量读取 blob,按请求顺序返回。
    ///
    /// 逐文件 `git show` 每个 stage 都要 spawn 一次进程(实测 ~12-16ms),
    /// 冲突文件多时进入 TUI 前的等待明显;这里改为请求-应答式复用一个
    /// 子进程,进程开销从 O(3N) 降为 O(1)。
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
        // 逐条请求-应答,避免双向管道同时写满导致的死锁
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
            // 应答头:`<oid> <type> <size>` 或 `<oid> missing`
            let mut header = String::new();
            stdout.read_line(&mut header)?;
            let size: usize = header
                .split_whitespace()
                .nth(2)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| failed(format!("对象 {oid} 不可读: {}", header.trim())))?;
            let mut content = vec![0u8; size];
            stdout.read_exact(&mut content)?;
            // 每个对象后跟一个换行分隔符
            stdout.read_exact(&mut [0u8; 1])?;
            blobs.push(content);
        }
        drop(stdin);
        child.wait()?;
        Ok(blobs)
    }

    /// 列出可作为 merge / rebase 目标的分支:本地 + 远程跟踪,
    /// 排除当前分支与 HEAD 符号引用。
    pub fn list_branches(&self) -> Result<Vec<String>, GitError> {
        let current = {
            let out = self.run_ok(&["branch", "--show-current"])?;
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let queries: [&[&str]; 2] = [
            &["branch", "--format=%(refname:short)"],
            &["branch", "-r", "--format=%(refname:short)"],
        ];
        let mut branches = Vec::new();
        for args in queries {
            let out = self.run_ok(args)?;
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let name = line.trim();
                if name.is_empty() || name == current || name.contains("HEAD") {
                    continue;
                }
                branches.push(name.to_owned());
            }
        }
        Ok(branches)
    }

    /// 最近提交列表(`--oneline` 行,首列为短 hash),提交选择器用。
    ///
    /// `others_only` 为 true 时只列不在当前分支上的提交(cherry-pick 候选),
    /// 否则列当前分支的最近提交(revert 候选)。
    /// 空仓库等无提交可列的场景返回空列表而非报错。
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

    /// 探测仓库体征:分支、改动数、贮藏数、待推送数与提交总数。
    ///
    /// 五个查询彼此独立,而每次 git 进程 spawn 约 12-16ms,串行累计的
    /// 延迟在打开菜单时可感知,因此用作用域线程并行执行,总耗时约等于
    /// 最慢的一次;`ahead` / `level` 在无上游 / 空仓库时查询会非零退出,
    /// 分别归一化为 None / 0 而非报错。
    pub fn vitals(&self) -> Result<RepoVitals, GitError> {
        // 汇合子线程;查询闭包只返回 Result 不会 panic,此分支仅为完备
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

    /// 将解决后的内容写入工作区文件并 `git add`。
    pub fn stage_resolved(&self, path: &str, content: &[u8]) -> Result<(), GitError> {
        std::fs::write(self.top.join(path), content)?;
        self.run_ok(&["add", "--", path])?;
        Ok(())
    }

    /// 继续当前 merge / rebase(冲突全部解决后调用)。
    ///
    /// 以透传模式执行:git 与钩子的输出(含颜色)实时流向用户终端。
    /// 返回退出码而非直接判错:rebase --continue 在下一个 commit
    /// 冲突时也会非零退出,是否算失败由调用方结合冲突探测决定。
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

    /// 中止当前 merge / rebase。
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

/// 解析 `git ls-files -u -z` 的输出。
///
/// 每个条目格式为 `<mode> <oid> <stage>\t<path>`,以 NUL 分隔;
/// 同一路径会按 stage 出现 1~3 次,归组为单个 [`ConflictedFile`]。
fn parse_ls_files_unmerged(text: &str) -> Vec<ConflictedFile> {
    let mut files: Vec<ConflictedFile> = Vec::new();
    for entry in text.split('\0').filter(|e| !e.is_empty()) {
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut fields = meta.split_whitespace().skip(1);
        let (Some(oid), Some(stage)) = (fields.next(), fields.next()) else {
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
        match stage {
            "1" => files[idx].base = Some(oid.to_owned()),
            "2" => files[idx].ours = Some(oid.to_owned()),
            "3" => files[idx].theirs = Some(oid.to_owned()),
            _ => {}
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unmerged_entries_grouped_by_path() {
        let text = "100644 aaaa 1\tsrc/a.rs\x00100644 bbbb 2\tsrc/a.rs\x00100644 cccc 3\tsrc/a.rs\x00100644 dddd 2\tREADME.md\x00100644 eeee 3\tREADME.md\x00";
        let files = parse_ls_files_unmerged(text);
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[0],
            ConflictedFile {
                path: "src/a.rs".to_owned(),
                base: Some("aaaa".to_owned()),
                ours: Some("bbbb".to_owned()),
                theirs: Some("cccc".to_owned()),
            }
        );
        // add/add 冲突:没有 stage 1
        assert!(files[1].base.is_none());
        assert!(files[1].ours.is_some() && files[1].theirs.is_some());
    }

    #[test]
    fn parses_empty_output() {
        assert!(parse_ls_files_unmerged("").is_empty());
    }

    #[test]
    fn tolerates_tab_in_path() {
        // -z 模式下路径不转义,首个 \t 之后整体视为路径
        let text = "100644 aaaa 2\ta\tb.txt\0";
        let files = parse_ls_files_unmerged(text);
        assert_eq!(files[0].path, "a\tb.txt");
    }
}
