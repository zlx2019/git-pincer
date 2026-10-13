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

/// 符号链接在 index 中的 mode。
const MODE_SYMLINK: u32 = 0o120000;
/// 子模块(gitlink)在 index 中的 mode。
const MODE_GITLINK: u32 = 0o160000;

/// 冲突文件某个 stage 在 index 中的条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// 文件 mode(如 0o100644 普通文件、0o120000 符号链接、0o160000 子模块)
    pub mode: u32,
    /// 对象 id:普通文件与符号链接为 blob,子模块为其提交
    pub oid: String,
}

/// 一个处于冲突状态的文件及其在 index 中的 stage 分布。
///
/// 各 stage 记录 mode 与 oid(缺失该 stage 时为 None),
/// 供 [`Git::read_blobs`] 单进程批量读取内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictedFile {
    /// 相对仓库根目录的路径
    pub path: String,
    /// stage 1(base);add/add 冲突时缺失
    pub base: Option<Stage>,
    /// stage 2(ours);本方删除了该文件时缺失
    pub ours: Option<Stage>,
    /// stage 3(theirs);对方删除了该文件时缺失
    pub theirs: Option<Stage>,
}

impl ConflictedFile {
    /// 是否涉及符号链接(任一 stage 为链接):链接目标不能按文本合并,
    /// 也不能经文件系统写入(会写穿到链接指向的文件),只能整条取用一侧。
    pub fn is_symlink(&self) -> bool {
        self.has_mode(MODE_SYMLINK)
    }

    /// 是否涉及子模块(任一 stage 为 gitlink):其提交在子模块仓库而非本仓库
    /// 的对象库中,无法读取内容,只能整条取用一侧记录的提交。
    pub fn is_submodule(&self) -> bool {
        self.has_mode(MODE_GITLINK)
    }

    /// 是否有任一 stage 为指定 mode。
    fn has_mode(&self, mode: u32) -> bool {
        [&self.base, &self.ours, &self.theirs]
            .into_iter()
            .flatten()
            .any(|stage| stage.mode == mode)
    }
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

/// 可切换的分支清单(switch 选择器用,按本地 / 远程分组)。
#[derive(Debug, Default)]
pub struct SwitchBranches {
    /// 本地分支(不含当前分支)
    pub locals: Vec<String>,
    /// 远程跟踪分支(不含 HEAD 符号引用与已有同名本地分支的项)
    pub remotes: Vec<String>,
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

    /// 列出某前缀下的分支引用(短名),符号引用(如 origin/HEAD)被剔除。
    ///
    /// 不用 `branch --format=%(refname:short)`:它对 `origin/HEAD` 的缩写
    /// 行为随 git 版本变化(旧版输出 `origin/HEAD`,新版输出裸 `origin`),
    /// 按名称启发过滤不可靠;`%(symref)` 字段可精确识别符号引用。
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

    /// 列出可切换的分支:本地分支排除当前;远程分支排除 HEAD 符号引用,
    /// 以及已有同名本地分支的项(如本地 `foo` 存在时隐藏 `origin/foo`,
    /// 本地条目已覆盖该目标,避免选择器里重复出现)。
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
        // 远程分支去掉首段远程名后与本地重名的隐藏(origin/feat/x → feat/x)
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

    /// 完整引用是否存在(如 `refs/heads/foo` / `refs/remotes/origin/foo`)。
    pub fn ref_exists(&self, full_ref: &str) -> Result<bool, GitError> {
        Ok(self
            .run(&["show-ref", "--verify", "--quiet", full_ref])?
            .status
            .success())
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
        let full = self.top.join(path);
        // 工作区路径是符号链接时先删除链接本身:fs::write 会跟随链接,
        // 把内容写进链接指向的文件(可能在仓库之外)
        if std::fs::symlink_metadata(&full).is_ok_and(|meta| meta.file_type().is_symlink()) {
            std::fs::remove_file(&full)?;
        }
        std::fs::write(&full, content)?;
        self.add_path(path)
    }

    /// 整条取用冲突文件的某一侧(`stage` 为 2 本地 / 3 远端),用于不能按内容
    /// 合并、也不能经文件系统写入的冲突(符号链接 / 子模块),做法与 git mergetool 一致:
    /// - 子模块:只改 index(`update-index`),不动子模块工作区(其中可能有未推送的提交)
    /// - 其余:该侧存在时 `checkout-index -f` 检出该侧再 `git add`(git 先删除旧路径
    ///   再创建,不会写穿链接,并遵循 core.symlinks);该侧已删除时 `git rm`
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

    /// 按字面路径 `git add`:文件名可能含 `*` `?` `[` 等 glob 字符,按 pathspec
    /// 解析会把匹配到的其他文件(含未跟踪文件)一并暂存进合并提交。
    fn add_path(&self, path: &str) -> Result<(), GitError> {
        self.run_ok(&["--literal-pathspecs", "add", "--", path])?;
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

    /// 普通文件的 stage 条目
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
        // add/add 冲突:没有 stage 1
        assert!(files[1].base.is_none());
        assert!(files[1].ours.is_some() && files[1].theirs.is_some());
    }

    /// mode 按八进制解析;任一 stage 为符号链接即视为链接冲突
    /// (merge-ort 会把链接 ↔ 文件的类型变更拆成两个路径,链接一侧可能只剩单个 stage)
    #[test]
    fn detects_symlink_stages() {
        let text = "120000 aaaa 1\tlink\x00120000 bbbb 2\tlink\x00120000 cccc 3\tlink\x00120000 dddd 2\tx\x00";
        let files = parse_ls_files_unmerged(text);
        assert_eq!(files[0].ours.as_ref().map(|s| s.mode), Some(MODE_SYMLINK));
        assert!(files[0].is_symlink());
        assert!(files[1].is_symlink() && files[1].theirs.is_none());
        // mode 非法的条目被跳过
        assert!(parse_ls_files_unmerged("12x000 aaaa 2\tbad\x00").is_empty());
    }

    /// 任一 stage 为 gitlink 即视为子模块冲突
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
        // -z 模式下路径不转义,首个 \t 之后整体视为路径
        let text = "100644 aaaa 2\ta\tb.txt\0";
        let files = parse_ls_files_unmerged(text);
        assert_eq!(files[0].path, "a\tb.txt");
    }
}
