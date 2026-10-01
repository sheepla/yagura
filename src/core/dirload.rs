//! Reading one directory level.
//!
//! This is the only place that turns filesystem entries into [`DirEntry`]
//! values. It is always called from a blocking task, never from the UI loop
//! (invariant I1).

use std::ffi::OsStr;
use std::io;
use std::path::Path;

use ignore::WalkBuilder;

use crate::core::fsmodel::{DirEntry, NodeKind};

/// Files that are never worth watching, regardless of the ignore files.
const JUNK_NAMES: &[&str] = &["4913", ".DS_Store"];

/// Junk patterns that do not need a glob engine.
const JUNK_SUFFIXES: &[&str] = &["~", ".swp"];

#[derive(Debug, thiserror::Error)]
pub enum DirError {
    #[error("cannot read directory: {0}")]
    Io(#[from] io::Error),
}

/// Lists the direct children of `dir`.
///
/// With `ignore_enabled` the `.gitignore` files, the global excludes and the
/// user level ignores are applied. `.git` itself is always skipped.
pub fn load_dir(dir: &Path, ignore_enabled: bool) -> Result<Vec<DirEntry>, DirError> {
    let mut entries = Vec::new();
    if ignore_enabled {
        let mut builder = WalkBuilder::new(dir);
        builder
            .max_depth(Some(1))
            .follow_links(false)
            .standard_filters(false)
            .hidden(false)
            .parents(false)
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            // Honour ignore files even outside a git repository: the snapshot
            // baseline works in plain directories too.
            .require_git(false)
            .build();
        for result in builder.build() {
            let entry = match result {
                Ok(entry) => entry,
                Err(err) => {
                    // An error with no depth belongs to the directory itself,
                    // so the caller has to hear about it. Deeper errors are a
                    // single bad entry and must not abort the listing.
                    if err.depth().is_none_or(|depth| depth == 0) {
                        return Err(DirError::Io(io::Error::other(err.to_string())));
                    }
                    tracing::debug!(?dir, %err, "walker error");
                    continue;
                }
            };
            if entry.depth() != 1 || is_skipped(entry.file_name()) {
                continue;
            }
            entries.push(classify(entry.path(), entry.file_name()));
        }
    } else {
        for result in std::fs::read_dir(dir)? {
            match result {
                Ok(entry) => {
                    if is_skipped(&entry.file_name()) {
                        continue;
                    }
                    entries.push(classify(&entry.path(), &entry.file_name()));
                }
                Err(err) => {
                    // One unreadable entry must not abort the whole listing.
                    // `io::Error` carries no path, so the entry is only logged.
                    tracing::debug!(?dir, %err, "read_dir entry error");
                }
            }
        }
    }
    Ok(entries)
}

/// True for entries yagura never shows: the git directory and editor leftovers.
pub fn is_skipped(name: &OsStr) -> bool {
    if name == OsStr::new(".git") {
        return true;
    }
    let name = name.to_string_lossy();
    if JUNK_NAMES.contains(&name.as_ref()) {
        return true;
    }
    if name.starts_with(".#") {
        return true;
    }
    JUNK_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// Determines the kind of a path without following symbolic links.
fn classify(path: &Path, name: &OsStr) -> DirEntry {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) => {
            tracing::debug!(?path, %err, "stat failed");
            return DirEntry {
                name: name.to_os_string(),
                kind: NodeKind::Other,
                link_target: None,
                unreadable: true,
            };
        }
    };
    let kind = if metadata.is_symlink() {
        NodeKind::Symlink
    } else if metadata.is_dir() {
        NodeKind::Dir
    } else if metadata.is_file() {
        NodeKind::File
    } else {
        NodeKind::Other
    };
    // A link is displayed as `name -> target` and never followed, which also
    // stops link cycles from expanding forever.
    let link_target = if kind == NodeKind::Symlink {
        std::fs::read_link(path).ok().map(|target| {
            target
                .file_name()
                .unwrap_or(target.as_os_str())
                .to_os_string()
        })
    } else {
        None
    };
    DirEntry {
        name: name.to_os_string(),
        kind,
        link_target,
        unreadable: false,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = load_dir(dir, true)
            .expect("load")
            .iter()
            .map(|entry| entry.name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn lists_direct_children_only() {
        let tmp = temp_dir();
        fs::create_dir_all(tmp.path().join("dir/inner")).expect("mkdir");
        fs::write(tmp.path().join("dir/inner/deep.txt"), "x").expect("write");
        fs::write(tmp.path().join("top.txt"), "x").expect("write");
        assert_eq!(
            names(tmp.path()),
            vec!["dir".to_string(), "top.txt".to_string()]
        );
    }

    #[test]
    fn honours_gitignore() {
        let tmp = temp_dir();
        fs::write(tmp.path().join(".gitignore"), "*.log\nbuild/\n").expect("write");
        fs::write(tmp.path().join("keep.rs"), "").expect("write");
        fs::write(tmp.path().join("noise.log"), "").expect("write");
        fs::create_dir(tmp.path().join("build")).expect("mkdir");
        assert_eq!(
            names(tmp.path()),
            vec![".gitignore".to_string(), "keep.rs".to_string()]
        );
    }

    #[test]
    fn no_ignore_shows_everything() {
        let tmp = temp_dir();
        fs::write(tmp.path().join(".gitignore"), "*.log\n").expect("write");
        fs::write(tmp.path().join("noise.log"), "").expect("write");
        let mut names: Vec<String> = load_dir(tmp.path(), false)
            .expect("load")
            .iter()
            .map(|entry| entry.name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![".gitignore".to_string(), "noise.log".to_string()]
        );
    }

    #[test]
    fn skips_git_and_editor_junk() {
        let tmp = temp_dir();
        fs::create_dir(tmp.path().join(".git")).expect("mkdir");
        for name in ["4913", ".DS_Store", "notes.txt~", "vim.swp", ".#42"] {
            fs::write(tmp.path().join(name), "").expect("write");
        }
        fs::write(tmp.path().join("main.rs"), "").expect("write");
        assert_eq!(names(tmp.path()), vec!["main.rs".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn marks_symlinks_without_following_them() {
        let tmp = temp_dir();
        let target = tmp.path().join("target");
        fs::create_dir(&target).expect("mkdir");
        std::os::unix::fs::symlink(&target, tmp.path().join("link")).expect("symlink");
        let entries = load_dir(tmp.path(), false).expect("load");
        let link_entry = entries
            .iter()
            .find(|entry| entry.name == OsStr::new("link"))
            .expect("link entry");
        assert_eq!(link_entry.kind, NodeKind::Symlink);
        assert_eq!(
            link_entry.link_target.as_deref(),
            Some(OsStr::new("target"))
        );
    }

    #[test]
    fn classifies_a_path_that_cannot_be_stat_as_unreadable() {
        let tmp = temp_dir();
        let missing = tmp.path().join("gone");
        let entry = classify(&missing, OsStr::new("gone"));
        assert!(entry.unreadable);
        assert_eq!(entry.kind, NodeKind::Other);
    }
}
