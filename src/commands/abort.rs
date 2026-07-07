//! Abort the merge / rebase in progress (asks for confirmation first).

use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::Result;

use crate::git::{Git, RepoState};

/// 运行 abort 子命令。
pub fn run(verbose: bool, dir: &Path) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    let state = git.state()?;
    if state == RepoState::Clean {
        println!("[git-peace] 当前没有进行中的 merge / rebase");
        return Ok(());
    }

    print!(
        "确定要中止当前 {} 吗?已解决的进度将丢失 [y/N] ",
        state.op_name()
    );
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    if answer.trim().eq_ignore_ascii_case("y") {
        git.abort_op(state)?;
        println!("[git-peace] ✔ 已中止 {}", state.op_name());
    } else {
        println!("[git-peace] 已取消");
    }
    Ok(())
}
