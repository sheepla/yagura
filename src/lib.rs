//! yagura - a TUI file tree watcher for observing what a coding agent is doing.
//!
//! The crate is organised around a Flux loop: input and background work produce
//! [`app::Action`] values, the store reduces them against the state with pure
//! functions, and the resulting [`app::Effect`]s run outside the UI thread.
//! The invariants this code is built on are listed in the design document.

pub mod app;
pub mod cli;
pub mod core;
pub mod features;
pub mod input;
pub mod terminal;
pub mod ui;
