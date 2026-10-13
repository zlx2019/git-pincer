# Contributing Guide

Thank you for contributing. Please read the following guidelines before submitting changes.

## Repository Layout

| Path | What it is |
|------|------------|
| `src/merge.rs` | diff3 core: two 2-way line diffs grouped into `Stable / Ours / Theirs / Agree / Conflict` chunks (pure logic, no I/O) |
| `src/app.rs` | Session state machine: per-chunk decisions and whole-file picks (pure logic, no terminal, no git) |
| `src/git.rs` | Thin wrapper around the native `git` binary; the only place that talks to git |
| `src/ui/` | ratatui rendering and the key-event loop; `keymap.rs` is the single source of truth for key bindings |
| `src/cli.rs`, `src/commands/` | clap definitions and orchestration; `commands/resolve.rs` is the resolve → `git add` → `--continue` loop |
| `src/config.rs`, `src/i18n.rs`, `locales/` | Optional `config.toml` and the English / Chinese message catalogs |
| `tests/` | Integration tests that build real git repositories in temp dirs |
| `examples/playground.rs` | Generates a throwaway repository with every conflict scenario for manual testing |
| `fixtures/` | Sample conflict-marked files for `git-pincer file` |
| `npm/` | Packaging scripts for the npm distribution |

`merge` and `app` are deliberately free of I/O so they can be unit-tested without a terminal or git. Keep it that way.

## Development Environment

### Rust Toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

The project pins its Rust version in `rust-toolchain.toml`. After entering the project directory, `rustup` will automatically install the required toolchain and components when needed. `git` must be on your `PATH`.

### Development Tools

```bash
cargo install --locked cargo-deny     # Dependency security and license auditing
cargo install --locked typos-cli      # Spell checking
cargo install --locked git-cliff      # Changelog generation
cargo install --locked cargo-nextest  # Enhanced test runner
pip install pre-commit                # Git pre-commit checks
```

### Enable Pre-Commit

```bash
pre-commit install
```

After installation, each `git commit` runs fmt, deny, typos, check, clippy and test checks. The commit only succeeds when all of them pass.

The fmt hook only *checks* formatting and does not rewrite files, so run `cargo fmt --all` yourself before committing.

## Local Checks

Before submitting changes, make sure the following commands pass:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features --no-tests pass
cargo deny check
typos
```

Run a single test with `cargo nextest run <name-filter>`, or only the integration tests with `cargo nextest run --test git_flow`.

### Trying Your Change

The interactive loop needs a real terminal, so verify UI changes by hand:

```bash
cp fixtures/conflict.txt /tmp/ && cargo run -- file /tmp/conflict.txt   # no git repository needed
cargo run --example playground                                          # prints a cheat sheet of test commands
```

## Conventions

- **No panics in production code.** `unwrap`, `expect`, `panic!` and `dbg!` are linted (and CI runs clippy with `-D warnings`); tests may use them. Every public item needs a doc comment.
- **All git interaction goes through `src/git.rs`.** Arguments are passed as arrays, never through a shell, and host `GIT_DIR`-style variables are scrubbed. Do not spawn `git` from elsewhere.
- **User-facing text goes through `i18n::tr` / `tr_f`.** Add every new key to **both** `locales/en.conf` and `locales/zh.conf` with identical `{placeholders}`; unit tests fail when the two catalogs drift apart.
- **Key bindings live in `src/ui/keymap.rs`.** Dispatch, the hint bar and the help overlay all derive from it. A new action also needs a README entry in the list of rebindable actions.
- **Integration tests isolate themselves from your machine.** Use repo-local git config (`commit.gpgsign=false`, `core.autocrlf=false`) and scrub `GIT_DIR`-style variables from every git you spawn; the pre-commit hook runs the suite inside a git hook, where those variables point at this repository.
- **Code comments follow the file you are editing.** Item doc comments in the existing code are written in Chinese and module-level `//!` docs in English.

## Commit Convention

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/) and **must be written in English**: the changelog tool drops commits that contain Chinese characters from the release notes.

| Type | Description |
|------|-------------|
| `feat:` | New feature |
| `fix:` | Bug fix |
| `docs:` | Documentation change |
| `refactor:` | Refactor that is neither a feature nor a bug fix |
| `perf:` | Performance improvement |
| `test:` | Test-related change |
| `build:` | Build system or dependency change |
| `ci:` | CI workflow change |
| `chore:` | Toolchain or miscellaneous maintenance |

Add a scope when it helps (`fix(git): ...`) and mark breaking changes with `!` (`feat(cli)!: ...`) or a `BREAKING CHANGE:` footer.

The changelog is generated by git-cliff from commit history, so keep commit messages consistent. Merge commits, `chore(version):` bumps and changelog-only `docs:` commits are left out. Preview what the next release will contain with:

```bash
git cliff --unreleased --strip header
```

## Branches And Pull Requests

- Create feature branches from `main`.
- Keep each pull request focused on a single topic.
- Pull requests must pass CI: lint (fmt, clippy), tests on Linux, macOS and Windows, deny and typos.
- A bug fix should come with a test that fails without it. If the defect cannot be reproduced in a test, say so in the pull request rather than shipping a test that passes either way.
