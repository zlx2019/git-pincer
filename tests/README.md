# Tests

Integration tests that exercise the crate as a whole (each `tests/*.rs` file is
compiled as its own crate and can only reach the crate's public API).

Suggested scope:

- End-to-end CLI runs: invoke the built binary and assert on exit code, stdout
  and stderr (e.g. with `assert_cmd` + `predicates`).
- Full-command scenarios spanning argument parsing, dispatch and output.

Tip: to unit-test command logic directly instead of spawning a process, expose
it through a library target (`src/lib.rs`) and call those functions from tests.

Run with `cargo nextest run` (or `cargo test`).
