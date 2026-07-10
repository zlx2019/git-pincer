//! git-pincer: a Git conflict-resolution tool that lives in your terminal.
//!
//! Provides an IDEA-style three-pane conflict resolution TUI (local | result | remote),
//! and can also launch `merge / rebase / pull / cherry-pick / revert` directly,
//! taking over the whole conflict-resolution flow that follows.
//!
//! Module overview:
//! - [`merge`][] — diff3 three-way merge core and conflict-marker parsing (pure logic, ported from toolkit-rs)
//! - [`git`][] — thin wrapper around the native git CLI (shell out; inherits all user config)
//! - [`i18n`][] — runtime language detection and message catalogs (locales/*.conf)
//! - [`config`][] — user configuration file (theme / key bindings / CLI-option defaults)
//! - [`app`][] — state machine of a conflict-resolution session (pure logic, terminal-free)
//! - [`ui`][] — ratatui three-pane rendering and the key-event loop
//! - [`cli`][] / [`commands`][] — clap subcommand definitions and orchestration

pub mod app;
pub mod cli;
pub mod commands;
pub mod config;
pub mod git;
pub mod i18n;
pub mod merge;
pub mod ui;
