# Examples

Runnable examples for this crate. Add a file such as `examples/demo.rs` and run
it with `cargo run --example demo`.

Typical uses:

- Show how to build and dispatch the `clap` command tree in isolation.
- Call a subcommand's logic directly as a library function.
- Illustrate a specific argument, flag or output format in a minimal setup.

## playground

Generate a throwaway git repository packed with every conflict scenario
(merge with text + binary conflicts, two-round rebase, cherry-pick, revert,
and a conflict-marked file for `file` mode), then follow the printed cheat
sheet to exercise the TUI manually:

```bash
cargo run --example playground            # defaults to /tmp/git-pincer-playground
cargo run --example playground -- <path>  # custom location
```

Re-running the example rebuilds the repository from scratch.
