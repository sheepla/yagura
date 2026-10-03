//! Work the reducer asks for, to be run outside the UI loop.
//!
//! An effect carries only data. Whatever it needs in order to run - the root
//! path, the ignore settings, the channel to answer on - lives in the runner,
//! which keeps the reducer free of handles and side effects.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::core::fsmodel::NodeId;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Read one directory level.
    LoadDir {
        node: NodeId,
        /// Incremented for every request for the same node; a result whose
        /// generation does not match is dropped.
        generation: u64,
        /// Path of the directory relative to the root.
        rel: PathBuf,
    },
    /// Emit `Action::Tick` once per second.
    ScheduleTick,
    /// Load a file for the preview pane.
    LoadPreview {
        /// Path of the file relative to the root.
        rel: PathBuf,
        /// Generation the request was issued with.
        generation: u64,
        /// How the body should be shown; a diff mode asks for a baseline.
        mode: crate::app::state::PreviewMode,
        /// Baseline build the diff is against. A build that moved on in the
        /// meantime has to answer with no baseline instead of a stale one.
        baseline_ver: u64,
    },
    /// Rebuild the baseline (git status for now, a snapshot scan later).
    BuildBaseline {
        /// Which baseline to build.
        kind: crate::core::baseline::Kind,
        /// Incremented for every rebuild; older results are dropped.
        ver: u64,
    },
    /// Watch, or stop watching, the directories that hold a listing (§7.1).
    WatchDirs {
        /// Directories relative to the root; the empty path is the root
        /// itself. Watching a directory twice changes nothing.
        add: Vec<PathBuf>,
        remove: Vec<PathBuf>,
    },
    /// Work out how some paths differ from the baseline (§7.3).
    ComputeStats {
        /// Which baseline the answer has to be against: the two kinds count
        /// differently, and only one of them is being asked about.
        kind: crate::core::baseline::Kind,
        /// When the changes were seen: a path that has just become changed
        /// takes this as the moment it changed (invariant I1).
        at: SystemTime,
        /// The paths to ask about, each with the generation of its request;
        /// an answer carrying another one is stale.
        paths: Vec<(PathBuf, u64)>,
    },
    /// Read `HEAD` once the repository has gone quiet (§8.2).
    CheckHead,
    /// Walk the whole tree for the file finder, answering in chunks (§13.2).
    ScanFiles {
        /// Generation the walk answers with; one the state does not hold is
        /// from a walk that has been superseded.
        generation: u64,
    },
}
