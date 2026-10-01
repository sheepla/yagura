//! The git baseline: the changed set of `git status` against `HEAD`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use super::ChangedEntry;
use crate::core::fsmodel::Status;

/// Why the git baseline could not be built.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum GitError {
    #[error("git not found")]
    NoGit,
    #[error("not a git repository")]
    NotARepo,
    #[error("git status failed: {0}")]
    Failed(String),
}

/// The result of [`build`].
#[derive(Debug)]
pub struct Head {
    /// `HEAD` as of the build, or `None` on an unborn branch.
    pub head_oid: Option<String>,
    /// Every file that differs from `HEAD`, relative to the root.
    pub changed: BTreeMap<PathBuf, ChangedEntry>,
}

/// Checks that `root` sits inside a git work tree.
///
/// `rev-parse --git-dir` understands gitfiles, so linked worktrees and
/// submodules answer for themselves.
pub fn probe(root: &Path) -> Result<(), GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-dir"])
        .output()
        .map_err(spawn_error)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(GitError::NotARepo)
    }
}

/// Reads the changed set and `HEAD` of the repository containing `root`.
pub fn build(root: &Path, now: SystemTime) -> Result<Head, GitError> {
    probe(root)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "status",
            "--porcelain=v2",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ])
        .output()
        .map_err(spawn_error)?;
    if !output.status.success() {
        return Err(GitError::Failed(stderr_line(&output)));
    }
    Ok(Head {
        head_oid: head_oid(root),
        changed: parse_status(&output.stdout, now),
    })
}

fn spawn_error(err: std::io::Error) -> GitError {
    if err.kind() == std::io::ErrorKind::NotFound {
        GitError::NoGit
    } else {
        GitError::Failed(crate::core::text::os_error(&err))
    }
}

fn stderr_line(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown error")
        .to_owned()
}

/// `HEAD`, or `None` while no commit has been made yet.
fn head_oid(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!oid.is_empty()).then_some(oid)
}

/// Parses `git status --porcelain=v2 -z` output.
///
/// Paths outside the root arrive as `../…` when the root is a subdirectory of
/// the repository; they belong to a part of the tree the user cannot see and
/// are dropped.
fn parse_status(bytes: &[u8], now: SystemTime) -> BTreeMap<PathBuf, ChangedEntry> {
    let mut changed = BTreeMap::new();
    let mut records = bytes.split(|byte| *byte == b'\0');
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let status = match record[0] {
            b'1' | b'2' => xy_status(record),
            // A merge conflict: both sides have content, so it diffs.
            b'u' => Status::Modified,
            b'?' => Status::Added,
            // Ignored entries only appear with `--ignored`.
            b'!' => continue,
            _ => continue,
        };
        // A rename record carries the original path as the next record.
        if record[0] == b'2' {
            records.next();
        }
        let fields = match record[0] {
            b'1' => 8,
            b'2' => 9,
            b'u' => 10,
            b'?' => 1,
            _ => continue,
        };
        let Some(path) = path_after(record, fields) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        let path = PathBuf::from(String::from_utf8_lossy(path).into_owned());
        if path.components().any(|part| part == Component::ParentDir) {
            continue;
        }
        changed.insert(
            path,
            ChangedEntry {
                status,
                changed_at: now,
            },
        );
    }
    changed
}

/// Reads the `<XY>` pair of an ordinary or rename record.
fn xy_status(record: &[u8]) -> Status {
    let xy = record.get(2..4).unwrap_or(b"M.");
    let index = xy[0];
    let worktree = xy[1];
    if index == b'D' || worktree == b'D' {
        Status::Deleted
    } else if index == b'A' || worktree == b'A' {
        Status::Added
    } else {
        Status::Modified
    }
}

/// Skips `fields` space separated fields and returns the rest, which is the
/// path: `-z` output never quotes, so a path may contain spaces itself.
fn path_after(record: &[u8], fields: usize) -> Option<&[u8]> {
    let mut rest = record;
    for _ in 0..fields {
        let index = rest.iter().position(|byte| *byte == b' ')?;
        rest = &rest[index + 1..];
    }
    Some(rest)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A repository with one commit, a `.gitignore` and two tracked files.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        git(dir.path(), &["init"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        // Keeps the tests independent from a signing setup on the machine.
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.path().join(".gitignore"), "*.log\n").expect("write");
        std::fs::write(dir.path().join("a.txt"), "one\n").expect("write");
        std::fs::write(dir.path().join("b.txt"), "two\n").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "initial"]);
        dir
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH
    }

    #[test]
    fn parses_records_and_keeps_paths_with_spaces() {
        let bytes = b"1 M. N... 100644 100644 100644 aaaa bbbb docs/my notes.txt\0\
                      ? un tracked.md\0\
                      1 D. N... 100644 100644 000000 aaaa bbbb gone.txt\0\
                      1 .D N... 100644 100644 000000 aaaa bbbb out.txt\0\
                      u UU N... 100644 100644 100644 100644 aaaa bbbb cccc conflicted.rs\0\
                      2 R. N... 100644 100644 100644 aaaa bbbb 100 new name.txt\0old name.txt\0\
                      ! ignored.log\0";
        let changed = parse_status(bytes, now());
        assert_eq!(
            changed[Path::new("docs/my notes.txt")].status,
            Status::Modified
        );
        assert_eq!(changed[Path::new("un tracked.md")].status, Status::Added);
        assert_eq!(changed[Path::new("gone.txt")].status, Status::Deleted);
        assert_eq!(changed[Path::new("out.txt")].status, Status::Deleted);
        assert_eq!(changed[Path::new("conflicted.rs")].status, Status::Modified);
        assert_eq!(changed[Path::new("new name.txt")].status, Status::Modified);
        assert!(!changed.contains_key(Path::new("old name.txt")));
        assert!(!changed.contains_key(Path::new("ignored.log")));
        assert_eq!(changed.len(), 6);
        assert_eq!(changed[Path::new("docs/my notes.txt")].changed_at, now());
    }

    #[test]
    fn entries_outside_the_root_are_dropped() {
        let bytes = b"1 M. N... 100644 100644 100644 aaaa bbbb ../outside.txt\0\
                      1 M. N... 100644 100644 100644 aaaa bbbb inside.txt\0";
        let changed = parse_status(bytes, now());
        assert_eq!(changed.len(), 1);
        assert!(changed.contains_key(Path::new("inside.txt")));
    }

    #[test]
    fn a_directory_that_is_not_a_repository_says_so() {
        let dir = tempfile::tempdir().expect("temp dir");
        let err = build(dir.path(), now()).expect_err("no repository");
        assert_eq!(err, GitError::NotARepo);
        assert_eq!(probe(dir.path()), Err(GitError::NotARepo));
    }

    #[test]
    fn modified_added_and_deleted_files_appear_with_head_oid() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "one\nchanged\n").expect("write");
        std::fs::remove_file(dir.path().join("b.txt")).expect("remove");
        std::fs::write(dir.path().join("new file.txt"), "x\n").expect("write");

        let head = build(dir.path(), now()).expect("build");
        assert!(head.head_oid.is_some());
        assert_eq!(head.changed[Path::new("a.txt")].status, Status::Modified);
        assert_eq!(head.changed[Path::new("b.txt")].status, Status::Deleted);
        assert_eq!(
            head.changed[Path::new("new file.txt")].status,
            Status::Added
        );
        assert_eq!(head.changed.len(), 3);
    }

    #[test]
    fn a_staged_file_is_added_and_ignored_files_stay_out() {
        let dir = repo();
        std::fs::write(dir.path().join("staged.txt"), "x\n").expect("write");
        git(dir.path(), &["add", "staged.txt"]);
        std::fs::write(dir.path().join("debug.log"), "noise\n").expect("write");

        let head = build(dir.path(), now()).expect("build");
        assert_eq!(head.changed[Path::new("staged.txt")].status, Status::Added);
        assert!(!head.changed.contains_key(Path::new("debug.log")));
        assert_eq!(head.changed.len(), 1);
    }

    #[test]
    fn an_empty_repository_has_no_head_but_lists_untracked_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        git(dir.path(), &["init"]);
        std::fs::write(dir.path().join("loose.txt"), "x\n").expect("write");

        let head = build(dir.path(), now()).expect("build");
        assert!(head.head_oid.is_none());
        assert_eq!(head.changed[Path::new("loose.txt")].status, Status::Added);
    }

    #[test]
    fn committing_clears_the_changed_set() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "one\nchanged\n").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "second"]);

        let head = build(dir.path(), now()).expect("build");
        assert!(head.changed.is_empty());
        assert!(head.head_oid.is_some());
    }
}
