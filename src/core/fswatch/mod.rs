//! Watching the filesystem for what a coding agent is doing.
//!
//! The watch itself lives in [`watcher`], the folding of its events into
//! batches in [`coalesce`]. Both are driven from outside the reducer: an
//! event never reaches the state on its own, only as an action (invariant I1).

pub mod coalesce;
pub mod watcher;

use std::path::PathBuf;

/// What a path did, as far as an event can tell.
///
/// The end state of a path is what matters, so only these three survive the
/// folding; renames and atomic writes arrive as a pair of them (§7.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangeKind {
    Create,
    Modify,
    Remove,
}

impl ChangeKind {
    /// What the watch list calls it (§9).
    pub fn label(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Modify => "modify",
            Self::Remove => "remove",
        }
    }
}

/// One path that changed inside one batch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FsChange {
    /// Path relative to the watched root.
    pub rel: PathBuf,
    pub kind: ChangeKind,
}
