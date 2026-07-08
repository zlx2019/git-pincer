//! Git-free single-file mode: parse a conflict-marked file and write the resolution back in place.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use crate::app::{FileEntry, FileMerge, Session};
use crate::merge::parse_conflict_file;
use crate::ui::{self, Outcome};

/// file 子命令参数。
#[derive(Debug, Args)]
pub struct FileArgs {
    /// 带 `<<<<<<< / ======= / >>>>>>>` 冲突标记的文件路径
    pub path: PathBuf,
}

/// 运行单文件冲突解决。
pub fn run(args: FileArgs, light: bool) -> Result<()> {
    let text = std::fs::read_to_string(&args.path)
        .with_context(|| format!("读取 {} 失败", args.path.display()))?;
    let result = parse_conflict_file(&text)?;
    println!(
        "[git-peace] 解析到 {} 处冲突,进入解决界面…",
        result.conflicts
    );

    let display = args.path.display().to_string();
    let merge = FileMerge::from_result(display.clone(), result, text.ends_with('\n'));
    let mut session = Session::new(vec![FileEntry::Text(merge)], "file".to_owned());

    let outcome = ui::run_session(
        &mut session,
        &mut |_path: &str, bytes: &[u8]| {
            std::fs::write(&args.path, bytes)?;
            Ok(())
        },
        light,
    )?;
    match outcome {
        Outcome::Completed => println!("[git-peace] ✔ 已写回 {display}"),
        Outcome::Quit => println!("[git-peace] 已退出,文件未修改"),
    }
    Ok(())
}
