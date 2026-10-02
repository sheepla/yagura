//! What the diff is compared against.

pub mod git;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::core::diff::DiffStat;
use crate::core::fsmodel::Status;

/// Which content the diff is compared against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The `HEAD` commit of the repository.
    Head,
    /// The contents when yagura started.
    Snapshot,
}

impl Kind {
    /// The other baseline, for the `B` toggle.
    pub fn other(self) -> Self {
        match self {
            Self::Head => Self::Snapshot,
            Self::Snapshot => Self::Head,
        }
    }
}

/// Where a baseline build got to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// A build is in flight; the previous result stays visible meanwhile.
    Loading,
    /// The baseline is ready to diff against.
    Ready,
    /// There is no baseline, e.g. outside a repository.
    Unavailable,
}

/// One file that differs from the baseline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChangedEntry {
    /// How the file differs, never [`Status::Unchanged`].
    pub status: Status,
    /// First time the app saw the file in this state.
    pub changed_at: SystemTime,
    /// The `+N -N ~N` against the baseline, `None` when the sides could not
    /// be counted: not text, not there, or past the budget (§10).
    pub stat: Option<DiffStat>,
}

/// The outcome of one baseline build.
#[derive(Clone, Debug)]
pub struct Built {
    /// The baseline the result belongs to.
    pub kind: Kind,
    pub phase: Phase,
    /// Whether the root sits in a git work tree; gates the `B` toggle.
    pub repo: bool,
    /// `HEAD` as of the build, or `None` on an unborn branch.
    pub head_oid: Option<String>,
    /// Every file that differs from the baseline, relative to the root.
    pub changed: BTreeMap<PathBuf, ChangedEntry>,
    /// Note for the status bar, e.g. why the baseline is unavailable.
    pub message: Option<String>,
}

/// Reads one file's baseline content.
pub trait BaselineSource: Send + Sync {
    /// The baseline content of a path relative to the root.
    fn blob(&self, rel: &Path) -> Blob;
}

/// The baseline content of one file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Blob {
    /// The bytes of the baseline version.
    Bytes(Vec<u8>),
    /// The file is not in the baseline at all.
    Absent,
    /// The baseline could not be read, so the diff has no side to compare.
    Unreadable,
}

/// Builds the baseline for `kind`.
pub fn build(root: &Path, kind: Kind, now: SystemTime) -> Built {
    match kind {
        Kind::Head => match git::build(root, now) {
            Ok(head) => Built {
                kind: Kind::Head,
                phase: Phase::Ready,
                repo: true,
                head_oid: head.head_oid,
                changed: head.changed,
                message: None,
            },
            Err(err) => Built {
                kind: Kind::Head,
                phase: Phase::Unavailable,
                // git ran far enough to talk about the root, so the toggle
                // may still be worth retrying.
                repo: matches!(err, git::GitError::Failed(_)),
                head_oid: None,
                changed: BTreeMap::new(),
                message: Some(err.to_string()),
            },
        },
        Kind::Snapshot => Built {
            kind: Kind::Snapshot,
            phase: Phase::Unavailable,
            repo: git::probe(root).is_ok(),
            head_oid: None,
            changed: BTreeMap::new(),
            message: Some("snapshot baseline is not available".to_owned()),
        },
    }
}
