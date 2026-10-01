//! Everything that can happen to the application.

use std::path::PathBuf;
use std::time::SystemTime;

use ratatui::crossterm::event::KeyEvent;

use super::command::Command;
use crate::core::dirload::DirError;
use crate::core::fsmodel::{DirEntry, NodeId};
use crate::core::preview::{Doc, PreviewError};

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
    /// The baseline finished rebuilding.
    BaselineReady {
        /// Build number the request carried; a mismatch means the result is
        /// stale and must be dropped (invariant I3).
        ver: u64,
        built: crate::core::baseline::Built,
    },
}
