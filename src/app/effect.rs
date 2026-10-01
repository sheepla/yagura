//! Work the reducer asks for, to be run outside the UI loop.
//!
//! An effect carries only data. Whatever it needs in order to run - the root
//! path, the ignore settings, the channel to answer on - lives in the runner,
//! which keeps the reducer free of handles and side effects.

use std::path::PathBuf;

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
    },
}
