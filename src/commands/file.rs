//! Git-free single-file mode: parse a conflict-marked file and write the resolution back in place.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use crate::app::{FileEntry, FileMerge, Session};
use crate::i18n::{tr, tr_f};
use crate::merge::parse_conflict_file;
use crate::ui::{self, Outcome};

/// Arguments for the file subcommand.
#[derive(Debug, Args)]
pub struct FileArgs {
    /// Path to a file containing `<<<<<<< / ======= / >>>>>>>` conflict markers
    pub path: PathBuf,
}

/// Resolve conflicts in a single file.
pub fn run(args: FileArgs, light: bool) -> Result<()> {
    let text = std::fs::read_to_string(&args.path)
        .with_context(|| format!("failed to read {}", args.path.display()))?;
    let result = parse_conflict_file(&text)?;
    println!(
        "[git-pincer] {}",
        tr_f("file.parsed", &[("n", &result.conflicts.to_string())])
    );

    let display = args.path.display().to_string();
    let merge = FileMerge::from_result(display.clone(), result, text.ends_with('\n'));
    let mut session = Session::new(vec![FileEntry::Text(merge)], "file".to_owned());

    let outcome = ui::run_session(
        &mut session,
        &mut |entry: &FileEntry| {
            std::fs::write(&args.path, entry.resolved_bytes())?;
            Ok(())
        },
        light,
    )?;
    match outcome {
        Outcome::Completed => println!(
            "[git-pincer] {}",
            tr_f("file.written", &[("path", &display)])
        ),
        Outcome::Quit => println!("[git-pincer] {}", tr("file.exited")),
    }
    Ok(())
}
