# git-pincer

[![CI](https://github.com/zlx2019/git-pincer/actions/workflows/ci.yml/badge.svg)](https://github.com/zlx2019/git-pincer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.96.0%2B-orange.svg)](https://www.rust-lang.org)

**English** | [简体中文](./README.zh-CN.md)

> An IDEA-style three-pane Git conflict resolver that lives in your terminal.

`git-pincer` takes the pain out of merge conflicts: launch `merge / rebase / pull / cherry-pick / revert` through it (or invoke it after any conflict happens), resolve every conflict chunk by chunk in a three-pane TUI, and let it drive `git add` + `--continue` loops until the repository is clean again — including multi-round rebase and cherry-pick sequences.

<!-- TODO: replace with a real screenshot / GIF -->

```text
 ⚑ MERGE ▏█████░░░░░ 2/5 files ▏src/app.rs ▏✗ 2 conflicts
╭ LOCAL · feature ────╮╭ RESULT ───────╮╭ REMOTE · main ─────╮
│  8 » fn new() {     ││  8 ⋯ pending ⋯││  8 « fn make() {   │
│    »   init()       ││               ││    «   setup()     │
│  ✓ resolved chunks lose their band, the gutter keeps ✓    │
│ ────── ⋯ 12 unchanged lines (press z to expand) ⋯ ──────  │
╰─────────────────────╯╰───────────────╯╰────────────────────╯
  h take local · l take remote · x ignore · e edit · ? help
```

## Features

- **Three-pane merge UI** — local | result | remote, with chunk bands colored by change type using IDEA semantics: blue = modified, green = added, gray = deleted, red = conflict. Bands fade as chunks get resolved.
- **Precise diff rendering** — delta-style word-level emphasis inside changed chunks, plus syntax highlighting (Maple theme via syntect, selected by file extension, gracefully disabled for huge files).
- **Full flow takeover** — after all files are resolved it runs `git add` and the matching `--continue`, re-probes, and loops until the repository is clean. Multi-commit cherry-picks and multi-round rebases just work.
- **Interactive action menu** — running bare `git-pincer` in a clean repository opens a menu: pick an action, then a branch (merge / rebase) or a commit (cherry-pick / revert). Failures pop an in-TUI dialog and return to the menu instead of exiting.
- **Broad conflict-source support** — merge, rebase, pull, cherry-pick, revert, `git am`, and even flows without a `--continue` such as `stash pop`, `checkout -m` or `apply --3way`.
- **Native git, zero magic** — everything shells out to your git binary (the same route lazygit and IDEA take), so credentials, hooks, merge strategies and rerere all follow your existing configuration. Arguments are passed as arrays (no shell, no injection), and host `GIT_DIR`-style variables are scrubbed so nested invocations from hooks cannot hijack the wrong repository.
- **Terminal-aware theming** — dark (Tokyo Night) and light (Maple Light) themes via `--theme <auto|dark|light>`, `COLORFGBG` auto-detection, and automatic xterm-256 quantization on terminals without truecolor support.
- **Sensible fallbacks** — binary conflicts degrade to whole-file pick-one; a git-free `file` mode parses conflict markers directly; non-TTY invocations fail with a readable message instead of a panic.

## Installation

Requires `git` on your `PATH`. Building from source requires Rust 1.96+.

```bash
# From the repository
cargo install --git https://github.com/zlx2019/git-pincer

# Or from a local clone
cargo install --path .
```

Prebuilt binaries for major platforms are attached to [GitHub Releases](https://github.com/zlx2019/git-pincer/releases) on tagged versions. A crates.io release is planned.

## Usage

```bash
git-pincer                      # conflicts present: take over and resolve them
                                # clean repo: open the interactive action menu
git-pincer merge <branch>       # run git merge, resolve conflicts if any
git-pincer rebase <branch>      # run git rebase, looping through every conflicted commit
git-pincer pull origin main     # arguments are passed straight to git pull
git-pincer cherry-pick <commit> # multiple commits / options are passed through
git-pincer revert <commit>      # run git revert and take over the conflicts
git-pincer file conflict.txt    # git-free: parse a conflict-marked file, write it back
git-pincer abort                # abort the operation in progress (with confirmation)
```

Global options:

| Option | Description |
| ------ | ----------- |
| `-C, --repo <PATH>` | Operate on the repository at `PATH` (defaults to the current directory) |
| `--theme <auto\|dark\|light>` | UI theme; `auto` inspects `COLORFGBG` and falls back to dark |
| `-v, --verbose` | Echo every git command being executed |

Try the TUI without a git repository:

```bash
cp fixtures/conflict.txt /tmp/ && git-pincer file /tmp/conflict.txt
```

### Keybindings

| Key | Action |
| --- | ------ |
| `h` / `←` | Take the local side (taking both sides of a conflict in order keeps both) |
| `l` / `→` | Take the remote side |
| `x` | Ignore the remaining pending sides of the current chunk (keep base) |
| `u` | Undo all decisions on the current chunk |
| `e` | Edit the current chunk in `$EDITOR` |
| `a` | Apply every non-conflicting change at once |
| `j` / `k` | Move to the next / previous change chunk |
| `n` / `p` | Jump to the next / previous unresolved conflict |
| `y` / `Y` | Copy the current chunk result / the whole file result |
| `H` / `L` | Copy the local / remote side of the current chunk |
| `w` | Write the file (auto-applies remaining non-conflict changes, then `git add`) |
| `Tab` | Switch to the next file |
| `z` | Fold / unfold unchanged regions |
| `q` | Quit (press twice if files are unfinished; the scene is kept) |
| `?` | Show the full key reference |

### Supported conflict sources

| Source | Detected via | Finished with |
| ------ | ------------ | ------------- |
| `git merge` / `git pull` | `MERGE_HEAD` | `git merge --continue` |
| `git rebase` | `rebase-merge` / `rebase-apply` | `git rebase --continue` (multi-round) |
| `git cherry-pick` | `CHERRY_PICK_HEAD` | `git cherry-pick --continue` (multi-round) |
| `git revert` | `REVERT_HEAD` | `git revert --continue` |
| `git am -3` | `rebase-apply/applying` | `git am --continue` |
| `stash pop` / `checkout -m` / `apply --3way` | unmerged index entries only | nothing to continue — resolving is enough |

## How it works

- **diff3 core** — two 2-way diffs (base→ours, base→theirs, Myers algorithm with a 500 ms timeout guard) are grouped by base-range collisions into chunks: stable, one-sided, agreed, or conflicting. The grouping is deliberately conservative: reporting one conflict too many beats merging something silently wrong.
- **Pure-logic session** — every chunk side is pending / applied / ignored; take order defines how content is stitched together, and `$EDITOR` edits override a chunk wholesale. Files containing NUL bytes degrade to a whole-file binary choice.
- **Thin git wrapper** — conflict contents are read from index stages 1/2/3; writes go through `git add`; repository state (merge / rebase / cherry-pick / revert / am) is probed from the git directory so the right `--continue` is always used.

## Development

```bash
cargo nextest run --all-features --no-tests pass   # tests
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for the toolchain setup, pre-commit hooks and commit conventions.

## Acknowledgements

Built with [ratatui](https://github.com/ratatui/ratatui), [similar](https://github.com/mitsuhiko/similar), [syntect](https://github.com/trishume/syntect) and [clap](https://github.com/clap-rs/clap). Visual design inspired by the IntelliJ IDEA merge tool, [delta](https://github.com/dandavison/delta) and [lazygit](https://github.com/jesseduffield/lazygit).

## License

Distributed under the [MIT](./LICENSE) license.
