//! The watch overlay: the log of what has changed under the root (§9).

pub mod reducer;
pub mod state;
pub mod view;

pub use state::{WatchEntry, WatchState};
