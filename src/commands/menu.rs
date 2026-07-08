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

    loop {
        let Some(action) = ui::pick("git-pincer", &actions, light, true)? else {
            return Ok(());
        };
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
                    ui::notice("提示", "没有可选的目标分支", light)?;
                    continue;
                }
                let title = if action == 1 {
                    "merge:选择要合并进来的分支"
                } else {
                    "rebase:选择变基目标分支"
                };
                let Some(idx) = ui::pick(title, &branches, light, false)? else {
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
                    ui::notice("提示", "没有可选的提交", light)?;
                    continue;
                }
                let title = if others_only {
                    "cherry-pick:选择要摘取的提交"
                } else {
                    "revert:选择要撤销的提交"
                };
                let Some(idx) = ui::pick(title, &commits, light, false)? else {
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

        // 执行:成功或冲突已解决则结束;非冲突失败弹框展示原因,回到菜单
        let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
        match run::try_launch(&refs, verbose, dir, light)? {
            None => return Ok(()),
            Some(reason) => {
                ui::notice(&format!("git {} 失败", cmd[0]), &reason, light)?;
            }
        }
    }
}
