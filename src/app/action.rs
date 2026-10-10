//! Everything that can happen to the application.

use std::path::PathBuf;
use std::time::SystemTime;

use ratatui::crossterm::event::KeyEvent;

use super::command::Command;
use crate::core::baseline::ChangedEntry;
use crate::core::dirload::DirError;
use crate::core::fsmodel::{DirEntry, NodeId};
use crate::core::preview::{Doc, PreviewError};

/// Paths one [`Action::FilesFound`] carries: a walk of a hundred thousand
/// files arrives as a few hundred small actions rather than one that fills
/// the channel (§13.2).
pub const SCAN_CHUNK: usize = 128;

/// A message for the store loop.
#[derive(Debug)]
pub enum Action {
    /// A key press, not yet resolved to a command.
    Key(KeyEvent),
    /// A key press that the keymap resolved.
    Command(Command),
    /// The terminal changed size.
    Resize { width: u16, height: u16 },
    /// A bracketed paste.
    Paste(String),
    /// The mouse wheel turned: a signed number of lines, five to a notch.
    MouseWheel { delta: isize },
    /// The left button went down on a column of the terminal.
    MouseClick { column: u16 },
    /// One second clock tick. Carries the time so that the reducer stays pure.
    Tick { now: SystemTime },
    /// Leave the application.
    Quit,
    /// A directory listing finished.
    DirLoaded {
        node: NodeId,
        /// Generation the request was issued with; a mismatch means the result
        /// is stale and must be dropped (invariant I3).
        generation: u64,
        result: Result<Vec<DirEntry>, DirError>,
    },
    /// The preview finished loading.
    PreviewLoaded {
        /// Path of the file relative to the root.
        rel: PathBuf,
        /// Generation the request was issued with; a mismatch means the result
        /// is stale and must be dropped (invariant I3).
        generation: u64,
        result: Result<Doc, PreviewError>,
    },
    /// The colours beyond the budget of a load arrived (design §9.3).
    PreviewHighlights {
        /// Path of the file relative to the root.
        rel: PathBuf,
        /// Generation the load was issued with; a mismatch means the colours
        /// are stale and must be dropped (invariant I3).
        generation: u64,
        /// The meaning of every line, walked to the end of the run.
        spans: Vec<Vec<crate::core::highlight::Span>>,
        /// Whether lines past the hard cap are still plain.
        truncated: bool,
    },
    /// The baseline finished rebuilding.
    BaselineReady {
        /// Build number the request carried; a mismatch means the result is
        /// stale and must be dropped (invariant I3).
        ver: u64,
        built: crate::core::baseline::Built,
    },
    /// A batch of filesystem changes, folded out of the watch (§7.2).
    FsBatch {
        /// The paths that changed, one entry per path however often it did.
        changes: Vec<crate::core::fswatch::FsChange>,
        /// When the batch was taken, so the reducer can stamp the rows
        /// without reading the clock (invariant I1).
        at: SystemTime,
    },
    /// So much happened at once that nothing was remembered: the tree has to
    /// be read again (§7.2).
    FsRescanNeeded,
    /// The status of some paths came back from the baseline (§7.3).
    StatsComputed { results: Vec<StatAnswer> },
    /// What `git rev-parse HEAD` answered (§8.2).
    HeadChecked { oid: Option<String> },
    /// One chunk of paths the finder's walk has found (§13.2).
    FilesFound {
        /// Walk the chunk belongs to; a mismatch means it is stale (I3).
        generation: u64,
        /// Paths relative to the root, in the order the walk found them.
        rels: Vec<PathBuf>,
    },
    /// The finder's walk has finished (§13.2).
    FilesScanDone {
        /// Walk that finished; a mismatch means it was superseded (I3).
        generation: u64,
    },
    /// One batch of hits from the `rg` run of `generation` (§13.3).
    GrepChunk {
        /// Run the hits belong to; a mismatch means they are stale (I3).
        generation: u64,
        /// Hits relative to the root, in the order `rg` reported them, which
        /// groups them by file.
        matches: Vec<crate::features::finder::grep::GrepMatch>,
    },
    /// The run of `generation` is over (§13.3).
    GrepDone {
        /// Run that finished; a mismatch means it was superseded (I3).
        generation: u64,
        /// The run was stopped at the limit, so the list is all there is.
        truncated: bool,
    },
    /// The run of `generation` could not take the pattern; the list says so
    /// rather than hiding it (§7).
    GrepFailed { generation: u64, message: String },
    /// `rg` is not installed: the overlay is shut and the message is shown
    /// on the status bar (§13.3).
    GrepMissing { generation: u64, message: String },
}

/// The status of one path an [`crate::app::effect::Effect::ComputeStats`]
/// asked about.
#[derive(Debug)]
pub struct StatAnswer {
    /// Path relative to the root.
    pub rel: PathBuf,
    /// Generation the request carried; a mismatch means the answer is stale
    /// and must be dropped (invariant I3).
    pub generation: u64,
    /// The new entry, or `None` when the path no longer differs at all.
    pub entry: Option<ChangedEntry>,
}
