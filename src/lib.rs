//! git-peace: a Git conflict-resolution tool that lives in your terminal.
//!
//! Provides an IDEA-style three-pane conflict resolution TUI (local | result | remote),
//! and can also launch `merge / rebase / pull` directly, taking over the whole
//! conflict-resolution flow that follows.
//!
//! Module overview:
//! - [`merge`][] — diff3 three-way merge core and conflict-marker parsing (pure logic, ported from toolkit-rs)
//! - [`git`][] — thin wrapper around the native git CLI (shell out; inherits all user config)
//! - [`app`][] — state machine of a conflict-resolution session (pure logic, terminal-free)
//! - [`ui`][] — ratatui three-pane rendering and the key-event loop
//! - [`cli`][] / [`commands`][] — clap subcommand definitions and orchestration

pub mod app;
pub mod cli;
pub mod commands;
pub mod git;
pub mod merge;
pub mod ui;
