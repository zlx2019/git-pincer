//! Abort the merge / rebase in progress (asks for confirmation first).

use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::Result;

use crate::git::{Git, RepoState};
use crate::i18n::{tr, tr_f};

/// 运行 abort 子命令。
pub fn run(verbose: bool, dir: &Path) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    let state = git.state()?;
    if state == RepoState::Clean {
        println!("[git-pincer] {}", tr("abort.none"));
        return Ok(());
    }

    print!("{} ", tr_f("abort.confirm", &[("op", state.op_name())]));
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    if answer.trim().eq_ignore_ascii_case("y") {
        git.abort_op(state)?;
        println!(
            "[git-pincer] {}",
            tr_f("abort.done", &[("op", state.op_name())])
        );
    } else {
        println!("[git-pincer] {}", tr("abort.cancelled"));
    }
    Ok(())
}
