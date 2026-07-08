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
    menu_loop(&git, verbose, dir, light)
}

/// 菜单主循环:一级选操作,二级选目标;二级取消回到一级。
///
/// 弹框与一二级菜单共享同一个 [`ui::MenuSession`],页面切换不闪屏;
/// 组装出命令后先结束会话恢复终端,再执行 git(输出实时透传)。
fn menu_loop(git: &Git, verbose: bool, dir: &Path, light: bool) -> Result<()> {
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

    // 上一轮 git 执行失败的原因,进入下一轮菜单时先弹框展示
    let mut last_error: Option<(String, String)> = None;
    // 一级菜单上次选中的操作,返回或重进菜单时光标停在原处
    let mut last_action = 0usize;
    loop {
        // 组装要执行的 git 命令;二级选择取消则回到一级菜单
        let cmd: Vec<String> = {
            let mut session = ui::MenuSession::open(light)?;
            if let Some((title, body)) = last_error.take() {
                session.notice(&title, &body)?;
            }
            loop {
                let Some(action) = session.pick("git-pincer", &actions, true, last_action)? else {
                    return Ok(());
                };
                last_action = action;
                match action {
                    0 => break vec!["pull".to_owned()],
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
                        break if action == 1 {
                            vec!["merge".to_owned(), "--no-edit".to_owned(), target]
                        } else {
                            vec!["rebase".to_owned(), target]
                        };
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
                        break if others_only {
                            vec!["cherry-pick".to_owned(), hash]
                        } else {
                            vec!["revert".to_owned(), "--no-edit".to_owned(), hash]
                        };
                    }
                }
            }
            // session 在此 drop,恢复终端
        };

        // 执行:成功或冲突已解决则结束;非冲突失败记下原因,下一轮弹框展示
        let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
        match run::try_launch(&refs, verbose, dir, light)? {
            None => return Ok(()),
            Some(reason) => last_error = Some((format!("git {} 失败", cmd[0]), reason)),
        }
    }
}
