//! Switch to another branch; remote branches get a local tracking branch first.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Result, bail};
use clap::Args;

use crate::git::{Git, GitError};
use crate::i18n::{tr, tr_f};
use crate::ui::{self, MenuItem};

/// switch 子命令参数。
#[derive(Debug, Args)]
pub struct SwitchArgs {
    /// 目标分支(本地名或远程名如 origin/foo);省略时打开交互式选择器
    pub branch: Option<String>,
}

/// 运行 switch 子命令:有参直切,无参弹出分支选择器。
pub fn run(args: SwitchArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    let target = match args.branch {
        Some(branch) => branch,
        None => match pick_branch(&git, light)? {
            Some(branch) => branch,
            // 用户在选择器中取消
            None => return Ok(()),
        },
    };
    let cmd = switch_cmd(&git, &target)?;
    let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
    println!("[git-pincer] $ git {}", cmd.join(" "));
    // 透传执行:成功与失败的原因都由 git 自己输出
    let status = git.run_inherit(&refs)?;
    if !status.success() {
        bail!("{}", tr_f("switch.failed", &[("branch", &target)]));
    }
    Ok(())
}

/// 组装切换命令:本地分支直切;远程跟踪分支用 `--track` 创建同名本地分支;
/// 其余原样交给 git(裸名由 git 自身 DWIM 建跟踪分支,或报错)。
pub fn switch_cmd(git: &Git, target: &str) -> Result<Vec<String>, GitError> {
    if git.ref_exists(&format!("refs/heads/{target}"))? {
        return Ok(vec!["switch".to_owned(), target.to_owned()]);
    }
    if git.ref_exists(&format!("refs/remotes/{target}"))? {
        return Ok(vec![
            "switch".to_owned(),
            "--track".to_owned(),
            target.to_owned(),
        ]);
    }
    Ok(vec!["switch".to_owned(), target.to_owned()])
}

/// 打开与主菜单一致的分支选择器;返回 None 表示用户取消。
fn pick_branch(git: &Git, light: bool) -> Result<Option<String>> {
    if !std::io::stdout().is_terminal() {
        bail!("{}", tr("common.need_tty_menu"));
    }
    let items = branch_items(&git.list_switch_branches()?);
    if items.is_empty() {
        println!("[git-pincer] {}", tr("menu.no_branches"));
        return Ok(None);
    }
    // session 在函数结束时 Drop 恢复终端,随后的 git 输出打在正常屏幕上
    let mut session = ui::MenuSession::open(light)?;
    let Some(idx) = session.pick(tr("menu.pick_switch"), &items, None, 0)? else {
        return Ok(None);
    };
    Ok(Some(items[idx].label.clone()))
}

/// 把分支清单组装成选择器条目:本地在前,远程(带 origin/ 前缀)在后。
pub(crate) fn branch_items(branches: &crate::git::SwitchBranches) -> Vec<MenuItem> {
    branches
        .locals
        .iter()
        .map(|b| MenuItem::new(b.clone(), ""))
        .chain(
            branches
                .remotes
                .iter()
                .map(|b| MenuItem::new(b.clone(), tr("menu.remote_mark"))),
        )
        .collect()
}
