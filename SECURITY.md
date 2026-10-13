# Security Policy

## Supported Versions

Security updates are provided only for the latest released version.

| Version | Supported |
|---------|-----------|
| Latest release | Yes |
| Historical versions | No |

## Reporting A Vulnerability

If you discover a security vulnerability, please do **not** report it through a public issue, because doing so may expose the vulnerability before a fix is available.

Please report it privately by email: **zero9501@outlook.com**

When reporting, include as much of the following information as possible:

- Vulnerability type and impact scope
- Reproduction steps or proof of concept (a script that builds a minimal repository is ideal)
- Affected versions, install method (cargo, npm or release binary) and operating system

We will confirm the report as soon as possible and disclose it publicly after a fix has been released.

## Scope

git-pincer runs inside your repository, reads files that other people committed, and writes the resolved result back into your working tree. Findings in these areas are especially welcome:

- **Writing resolved files** — anything that lets a crafted repository (unusual paths, symlinks, glob characters in file names, submodules) make git-pincer write outside the repository, overwrite a file other than the conflicted one, or stage files the user did not resolve.
- **Command execution** — all git interaction shells out to the `git` binary with an argument array (never through a shell), and host `GIT_DIR`-style variables are scrubbed. Any path that lets repository content influence which command runs, or run in a different repository than the one in use, is in scope. The same applies to the editor launched by the `e` key.
- **Rendering untrusted content** — conflicted files are displayed in the terminal. Control characters or escape sequences in file content or file names that affect the terminal beyond the intended display are in scope.
- **Configuration parsing** — a malformed `config.toml` should fail with a readable error; anything beyond that (crash on valid input, unexpected side effects) is in scope.
- **Release artifacts** — integrity of the published crates.io, npm and GitHub Release artifacts. Every release archive ships with a `.sha256` checksum file.

## Known Limitations

These are documented behaviours rather than vulnerabilities, but they define the security boundary:

- **git-pincer inherits your git configuration.** Credentials, hooks, merge strategies and `rerere` all apply, and your `pre-commit` / `commit-msg` hooks run when git-pincer drives `git ... --continue`.
- **The configuration file is trusted.** The `[ui] editor` setting (like `$VISUAL` / `$EDITOR`) names a program that is executed when you press `e`. Protect `config.toml` with normal file permissions.
- **Copying to the clipboard** uses `pbcopy`, `xclip` or `wl-copy` when available and otherwise asks the terminal to set the clipboard through an OSC 52 escape sequence.
