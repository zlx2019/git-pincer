//! 裸 `git-pincer` 入口:有冲突现场直接接管,仓库干净时弹出交互式操作菜单。

use std::io::IsTerminal;
use std::path::Path;

use anyhow::Result;

use crate::git::{Git, RepoState};
use crate::i18n::{tr, tr_f};
use crate::ui::{self, MenuItem};

use super::resolve::{resolve_from_menu, resolve_loop};
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
        println!("{}", tr("menu.no_conflicts"));
        return Ok(());
    }
    menu_loop(&git, light)
}

/// 菜单主循环:一级选操作,二级选目标;二级取消回到一级。
///
/// 选择、执行与成功 / 失败弹框全部在同一个 [`ui::MenuSession`] 内完成
/// (git 以捕获方式执行,无终端交互),页面切换与结果反馈都不闪屏;
/// 产生冲突时把终端所有权移交给解决界面,全程不退出 alternate screen,
/// 捕获的 git 输出在首个解决会话结束后补进滚动历史。
fn menu_loop(git: &Git, light: bool) -> Result<()> {
    let actions: Vec<MenuItem> = [
        ("pull", tr("menu.pull_desc"), tr("menu.pull_hint")),
        ("merge", tr("menu.merge_desc"), tr("menu.merge_hint")),
        ("rebase", tr("menu.rebase_desc"), tr("menu.rebase_hint")),
        (
            "cherry-pick",
            tr("menu.cherry_desc"),
            tr("menu.cherry_hint"),
        ),
        ("revert", tr("menu.revert_desc"), tr("menu.revert_hint")),
    ]
    .into_iter()
    .map(|(label, desc, hint)| MenuItem::new(label, desc).with_hint(hint))
    .collect();

    // 一级菜单上次选中的操作,从二级返回时光标停在原处
    let mut last_action = 0usize;
    let (cmd, out, terminal) = {
        let mut session = ui::MenuSession::open(light)?;
        loop {
            // 每轮重新探测仓库体征,保证执行过命令后状态窗数据仍然准确
            let vitals = git.vitals()?;
            let Some(action) = session.pick("", &actions, Some(&vitals), last_action)? else {
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
                        session.notice(tr("menu.notice_info"), tr("menu.no_branches"))?;
                        continue;
                    }
                    let title = if action == 1 {
                        tr("menu.pick_merge")
                    } else {
                        tr("menu.pick_rebase")
                    };
                    let Some(idx) = session.pick(title, &branches, None, 0)? else {
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
                        session.notice(tr("menu.notice_info"), tr("menu.no_commits"))?;
                        continue;
                    }
                    let title = if others_only {
                        tr("menu.pick_cherry")
                    } else {
                        tr("menu.pick_revert")
                    };
                    let Some(idx) = session.pick(title, &commits, None, 0)? else {
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
            // 会话内捕获执行:成功 / 失败都弹框反馈后回到一级菜单,全程不退出 TUI
            session.flash(&format!("$ git {}", cmd.join(" ")))?;
            let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
            match run::launch_captured(git, &refs)? {
                run::LaunchOutcome::Failed(reason) => {
                    session.notice(&tr_f("menu.failed", &[("cmd", &cmd[0])]), &reason)?;
                }
                run::LaunchOutcome::Success(out) => {
                    let body = compose_notice(
                        &String::from_utf8_lossy(&out.stdout),
                        &String::from_utf8_lossy(&out.stderr),
                    );
                    session.notice(&tr_f("menu.done", &[("cmd", &cmd[0])]), &body)?;
                }
                // 冲突:移交终端所有权,现场直通冲突解决界面
                run::LaunchOutcome::Conflicts(out) => break (cmd, out, session.into_terminal()?),
            }
        }
    };

    // 在移交的终端上直接进入解决界面(不退出 alternate screen,不闪屏);
    // 命令历史与捕获的 git 输出由 resolve_from_menu 在恢复终端后补打
    resolve_from_menu(git, light, terminal, &cmd.join(" "), &out)
}

/// 成功弹框正文的最大行数,超出时只保留末尾。
const NOTICE_LINES: usize = 15;

/// 把捕获的 git 输出压缩成弹框正文:顺序合并 stdout / stderr,过长时截去开头。
fn compose_notice(stdout: &str, stderr: &str) -> String {
    let mut text = stdout.trim_end().to_owned();
    let err = stderr.trim_end();
    if !err.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err);
    }
    if text.trim().is_empty() {
        return tr("menu.no_output").to_owned();
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > NOTICE_LINES {
        let skipped = lines.len() - NOTICE_LINES;
        format!(
            "{}\n{}",
            tr_f("menu.skipped", &[("n", &skipped.to_string())]),
            lines[skipped..].join("\n")
        )
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空输出时给出占位提示
    #[test]
    fn compose_notice_empty_output() {
        assert_eq!(compose_notice("", "  \n"), "(no output)");
    }

    /// stdout 与 stderr 顺序合并,去掉尾部空白
    #[test]
    fn compose_notice_merges_streams() {
        assert_eq!(
            compose_notice("Already up to date.\n", "warning: x\n"),
            "Already up to date.\nwarning: x"
        );
    }

    /// 超长输出只保留末尾并标注省略行数
    #[test]
    fn compose_notice_truncates_long_output() {
        let long = (1..=20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let got = compose_notice(&long, "");
        assert!(got.starts_with("… (first 5 lines omitted)"));
        assert!(got.ends_with("line20"));
        assert_eq!(got.lines().count(), NOTICE_LINES + 1);
    }
}
