//! 裸 `git-pincer` 入口:有冲突现场直接接管,仓库干净时弹出交互式操作菜单。

use std::io::IsTerminal;
use std::path::Path;

use anyhow::Result;

use crate::git::{Git, RepoState};
use crate::ui::{self, MenuItem};

use super::resolve::resolve_loop;
use super::run;

/// 提交选择器最多展示的提交数。
const COMMIT_LIMIT: usize = 50;

/// 运行裸命令入口:接管现场或进入菜单。
pub fn run(verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    // 有冲突现场直接接管,保持「撞冲突后直接敲 git-pincer」的既有体验
    if !git.conflicted_files()?.is_empty() || git.state()? != RepoState::Clean {
        return resolve_loop(&git, light);
    }
    // 脚本 / 管道环境不弹菜单,保持静默安全
    if !std::io::stdout().is_terminal() {
        println!("Currently, there are no conflicts that need to be resolved.");
        return Ok(());
    }
    menu_loop(&git, light)
}

/// 菜单主循环:一级选操作,二级选目标;二级取消回到一级。
///
/// 选择、执行与失败弹框全部在同一个 [`ui::MenuSession`] 内完成
/// (git 以捕获方式执行,无终端交互),页面切换与失败反馈都不闪屏;
/// 只有成功或产生冲突才结束会话,回放捕获的 git 输出并接管后续。
fn menu_loop(git: &Git, light: bool) -> Result<()> {
    let actions: Vec<MenuItem> = [
        ("pull", "拉取远端"),
        ("merge", "合并分支"),
        ("rebase", "变基分支"),
        ("cherry-pick", "摘取提交"),
        ("revert", "撤销提交"),
    ]
    .into_iter()
    .map(|(label, desc)| MenuItem::new(label, desc))
    .collect();

    // 一级菜单上次选中的操作,从二级返回时光标停在原处
    let mut last_action = 0usize;
    let (cmd, outcome) = {
        let mut session = ui::MenuSession::open(light)?;
        loop {
            let Some(action) = session.pick("", &actions, true, last_action)? else {
                return Ok(());
            };
            last_action = action;
            // 组装要执行的 git 命令;二级选择取消则回到一级菜单
            let cmd: Vec<String> = match action {
                0 => vec!["pull".to_owned()],
                1 | 2 => {
                    let branches: Vec<MenuItem> = git
                        .list_branches()?
                        .into_iter()
                        .map(|b| MenuItem::new(b, ""))
                        .collect();
                    if branches.is_empty() {
                        session.notice("提示", "没有可选的目标分支")?;
                        continue;
                    }
                    let title = if action == 1 {
                        "merge:选择要合并进来的分支"
                    } else {
                        "rebase:选择变基目标分支"
                    };
                    let Some(idx) = session.pick(title, &branches, false, 0)? else {
                        continue;
                    };
                    let target = branches[idx].label.clone();
                    if action == 1 {
                        vec!["merge".to_owned(), "--no-edit".to_owned(), target]
                    } else {
                        vec!["rebase".to_owned(), target]
                    }
                }
                _ => {
                    let others_only = action == 3;
                    // --oneline 行拆为「短 hash + 标题」两列
                    let commits: Vec<MenuItem> = git
                        .recent_commits(others_only, COMMIT_LIMIT)?
                        .iter()
                        .filter_map(|line| line.split_once(' '))
                        .map(|(hash, subject)| MenuItem::new(hash, subject))
                        .collect();
                    if commits.is_empty() {
                        session.notice("提示", "没有可选的提交")?;
                        continue;
                    }
                    let title = if others_only {
                        "cherry-pick:选择要摘取的提交"
                    } else {
                        "revert:选择要撤销的提交"
                    };
                    let Some(idx) = session.pick(title, &commits, false, 0)? else {
                        continue;
                    };
                    let hash = commits[idx].label.clone();
                    if others_only {
                        vec!["cherry-pick".to_owned(), hash]
                    } else {
                        vec!["revert".to_owned(), "--no-edit".to_owned(), hash]
                    }
                }
            };
            // 会话内捕获执行:失败弹框后回到一级菜单,全程不退出 TUI
            session.flash(&format!("$ git {} 执行中…", cmd.join(" ")))?;
            let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
            match run::launch_captured(git, &refs)? {
                run::LaunchOutcome::Failed(reason) => {
                    session.notice(&format!("git {} 失败", cmd[0]), &reason)?;
                }
                outcome => break (cmd, outcome),
            }
        }
        // session 在此 drop,恢复终端
    };

    // 已回到常规终端:补一行命令历史并回放捕获的 git 输出
    println!("[git-pincer] $ git {}", cmd.join(" "));
    match outcome {
        run::LaunchOutcome::Success(out) => {
            replay(&out);
            Ok(())
        }
        run::LaunchOutcome::Conflicts(out) => {
            replay(&out);
            resolve_loop(git, light)
        }
        run::LaunchOutcome::Failed(_) => unreachable!("失败路径已在会话内处理"),
    }
}

/// 把捕获的 git 输出原样回放到终端,保留在滚动历史里。
fn replay(out: &std::process::Output) {
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
}
