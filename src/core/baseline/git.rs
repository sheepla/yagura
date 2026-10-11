//! The git baseline: the changed set of `git status` against `HEAD`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::time::SystemTime;

use super::{BaselineSource, Blob, ChangedEntry};
use crate::core::diff::{Diff, DiffStat};
use crate::core::fsmodel::Status;
use crate::core::preview::{BINARY_SNIFF_BYTES, MAX_BYTES};
use crate::core::text;

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

/// A `git` command pointed at the repository containing `root`.
///
/// `GIT_DIR`, `GIT_WORK_TREE` and friends left over from the caller's own
/// shell would make `git -C root` operate on the wrong repository, so they
/// are dropped before the child starts.
fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(root);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(name);
    }
    command
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
    let output = git_command(root)
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
    let output = git_command(root)
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
    let head_oid = head_oid(root);
    let mut changed = parse_status(&output.stdout, now);
    // The tree column counts every changed file: one reader answers the
    // HEAD side of all of them (§10). Without a HEAD the sides cannot be
    // counted, and the entries stay without a number.
    if head_oid.is_some()
        && !changed.is_empty()
        && let Ok(source) = GitSource::open(root)
    {
        for (rel, entry) in changed.iter_mut() {
            entry.stat = stat_of(&source, root, rel, entry.status);
        }
    }
    Ok(Head { head_oid, changed })
}

/// How many bytes of pathspecs one `git status` argument list may hold.
///
/// Past this the whole work tree is asked about instead: a single `git
/// status` is cheaper than one per few hundred paths, and the answer is
/// filtered back down to what was asked for.
const PATHSPEC_BUDGET: usize = 8_000;

/// The status of the given paths against `HEAD` (§7.3).
///
/// One answer per requested path, in the order it was asked for; a path
/// that no longer differs at all is answered with `None`, which is what
/// takes it back out of the changed set. An empty result means nothing
/// could be worked out - no `git`, no repository, a command that failed -
/// and the caller then leaves what it has alone.
pub fn stats(
    root: &Path,
    paths: &[PathBuf],
    at: SystemTime,
) -> Vec<(PathBuf, Option<ChangedEntry>)> {
    if paths.is_empty() {
        return Vec::new();
    }
    let output = match status(root, paths) {
        Some(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    let reported = parse_status(&output.stdout, at);
    // The counts are read through one process, and only while there is a
    // `HEAD` to count against: without one the two sides cannot be compared.
    let source = head_oid(root)
        .is_some()
        .then(|| GitSource::open(root).ok())
        .flatten();
    paths
        .iter()
        .map(|rel| {
            let entry = reported.get(rel).map(|found| ChangedEntry {
                status: found.status,
                changed_at: at,
                stat: source
                    .as_ref()
                    .and_then(|source| stat_of(source, root, rel, found.status)),
            });
            (rel.clone(), entry)
        })
        .collect()
}

/// `git status` for the given paths, or for the whole work tree when they
/// do not fit on one command line.
fn status(root: &Path, paths: &[PathBuf]) -> Option<std::process::Output> {
    let specs: Vec<String> = paths
        .iter()
        .map(|rel| format!(":(literal){}", rel.to_string_lossy().replace('\\', "/")))
        .collect();
    let mut command = git_command(root);
    command.args([
        "status",
        "--porcelain=v2",
        "-z",
        "--no-renames",
        "--untracked-files=all",
    ]);
    if specs.iter().map(String::len).sum::<usize>() <= PATHSPEC_BUDGET {
        command.arg("--");
        command.args(specs);
    }
    command.output().map_err(spawn_error).ok()
}

/// The `+N -N ~N` of one changed file against `HEAD`, or `None` when the
/// two sides are not text, not there, or past the budget (§10).
fn stat_of(source: &GitSource, root: &Path, rel: &Path, status: Status) -> Option<DiffStat> {
    let base = match source.blob(rel) {
        Blob::Bytes(bytes) => bytes,
        // A new file has nothing on the HEAD side.
        Blob::Absent if status == Status::Added => Vec::new(),
        Blob::Absent | Blob::Unreadable => return None,
    };
    let current = if status == Status::Deleted {
        Vec::new()
    } else {
        read_capped(&root.join(rel))?
    };
    // Binary content has no lines to count, and the pane refuses to draw a
    // diff of it either.
    if base[..base.len().min(BINARY_SNIFF_BYTES)].contains(&0)
        || current[..current.len().min(BINARY_SNIFF_BYTES)].contains(&0)
    {
        return None;
    }
    let base_text = text::sanitize(&String::from_utf8_lossy(&base));
    let current_text = text::sanitize(&String::from_utf8_lossy(&current));
    Diff::compute(&base_text, &current_text)
        .ok()
        .map(|diff| diff.stat)
}

/// The bytes of a file on disk, `None` when it is unreadable or past
/// [`MAX_BYTES`], which is where the pane stops reading as well.
fn read_capped(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_BYTES).then_some(bytes)
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
pub fn head_oid(root: &Path) -> Option<String> {
    let output = git_command(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!oid.is_empty()).then_some(oid)
}

/// Reads committed file contents through one long-lived `git cat-file`.
///
/// One process answers every request, so a diff of many files costs one
/// process instead of one per file. The reader is reached through
/// [`BaselineSource`] and dropped with the build it belongs to.
pub struct GitSource {
    /// `git rev-parse --show-prefix`: where the root sits inside the
    /// repository, e.g. `src/`. Object names are repository relative.
    prefix: String,
    batch: Mutex<Batch>,
}

impl GitSource {
    /// Starts a reader for the repository containing `root`.
    pub fn open(root: &Path) -> std::io::Result<GitSource> {
        let prefix = git_command(root)
            .args(["rev-parse", "--show-prefix"])
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .trim_end()
                    .to_owned()
            })
            .unwrap_or_default();
        let mut child = git_command(root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        Ok(GitSource {
            prefix,
            batch: Mutex::new(Batch {
                child,
                stdin,
                stdout: BufReader::new(stdout),
                alive: true,
            }),
        })
    }

    /// The `HEAD:<path>` name of a root relative path, or `None` when the
    /// path could not be turned into one safely.
    fn spec(&self, rel: &Path) -> Option<String> {
        let mut parts = Vec::new();
        for part in rel.components() {
            match part {
                Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
                Component::CurDir => {}
                // A path out of the root, absolute or with `..`, names a
                // file the user cannot see in this tree.
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        if parts.is_empty() {
            return None;
        }
        let name = format!("{}{}", self.prefix, parts.join("/"));
        // A newline would split the request in two and leave the stream one
        // answer short for every request after it.
        if name.contains(['\n', '\r']) {
            return None;
        }
        Some(format!("HEAD:{name}"))
    }
}

impl BaselineSource for GitSource {
    fn blob(&self, rel: &Path) -> Blob {
        let Some(spec) = self.spec(rel) else {
            return Blob::Unreadable;
        };
        match self.batch.lock() {
            Ok(mut batch) => batch.ask(&spec),
            Err(_) => Blob::Unreadable,
        }
    }
}

/// The `git cat-file --batch` process and its pipes.
struct Batch {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// False once an answer did not arrive: the stream can no longer be
    /// trusted to line answers up with requests.
    alive: bool,
}

impl Batch {
    /// Asks for one object and reads its answer.
    fn ask(&mut self, spec: &str) -> Blob {
        if !self.alive {
            return Blob::Unreadable;
        }
        let sent = self
            .stdin
            .write_all(spec.as_bytes())
            .and_then(|()| self.stdin.write_all(b"\n"))
            .and_then(|()| self.stdin.flush());
        if sent.is_err() {
            self.alive = false;
            return Blob::Unreadable;
        }
        self.read_object()
    }

    /// Reads one `<oid> blob <size>` frame, or a `<spec> missing` answer.
    fn read_object(&mut self) -> Blob {
        let mut header = String::new();
        match self.stdout.read_line(&mut header) {
            Ok(0) | Err(_) => {
                self.alive = false;
                return Blob::Unreadable;
            }
            Ok(_) => {}
        }
        let header = header.trim_end_matches(['\r', '\n']);
        // The answer to a name git cannot resolve is `<spec> missing`.
        if header == "missing" || header.ends_with(" missing") {
            return Blob::Absent;
        }
        // The last field of the header is the byte count of the frame.
        let Some(size) = header
            .rsplit(' ')
            .next()
            .and_then(|last| last.parse::<usize>().ok())
        else {
            self.alive = false;
            return Blob::Unreadable;
        };
        // A header is only a number from the other side of a pipe; one that
        // asks for more than the pane would ever read is not obeyed.
        if size > MAX_BYTES {
            self.alive = false;
            return Blob::Unreadable;
        }
        // The content is followed by one newline the protocol adds itself.
        let mut frame = vec![0; size + 1];
        if self.stdout.read_exact(&mut frame).is_err() {
            self.alive = false;
            return Blob::Unreadable;
        }
        frame.truncate(size);
        Blob::Bytes(frame)
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
                // Filled in by [`build`] once the two sides are read.
                stat: None,
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
    use std::time::Duration;

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
    fn a_linked_worktree_answers_as_a_repository() {
        // The `.git` of a linked worktree is a file naming a git directory
        // elsewhere; `rev-parse --git-dir` reads through it, and the status
        // of the linked working tree is the status of that repository (§12).
        let main = repo();
        let holder = tempfile::tempdir().expect("temp dir");
        let linked = holder.path().join("linked");
        git(
            main.path(),
            &[
                "worktree",
                "add",
                "-b",
                "side",
                linked.to_str().expect("a path git can read"),
                "HEAD",
            ],
        );
        assert!(
            linked.join(".git").is_file(),
            "a linked worktree carries a gitfile rather than a directory"
        );

        probe(&linked).expect("the linked worktree is a repository");

        std::fs::write(linked.join("a.txt"), "changed\n").expect("write");
        let head = build(&linked, now()).expect("build");
        assert_eq!(head.changed[Path::new("a.txt")].status, Status::Modified);
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

    #[test]
    fn statuses_and_diff_stats_match_a_real_working_tree() {
        let dir = repo();
        // An ignore rule outside the tree, so that deleting `.gitignore`
        // below still counts as a change of its own.
        std::fs::write(dir.path().join(".git/info/exclude"), "*.log\n").expect("exclude");
        // Modified, unstaged: one line added.
        std::fs::write(dir.path().join("a.txt"), "one\nchanged\n").expect("write");
        // Modified, staged: one line added.
        std::fs::write(dir.path().join("b.txt"), "two\nthree\n").expect("write");
        git(dir.path(), &["add", "b.txt"]);
        // Deleted, unstaged.
        std::fs::remove_file(dir.path().join(".gitignore")).expect("remove");
        // Untracked.
        std::fs::write(dir.path().join("loose.txt"), "x\ny\n").expect("write");
        // Staged and new.
        std::fs::write(dir.path().join("staged.txt"), "s\n").expect("write");
        git(dir.path(), &["add", "staged.txt"]);
        // Ignored: never part of the changed set.
        std::fs::write(dir.path().join("debug.log"), "noise\n").expect("write");

        let head = build(dir.path(), now()).expect("build");
        let count = |name: &str| {
            head.changed
                .get(Path::new(name))
                .and_then(|entry| entry.stat)
                .unwrap_or_else(|| panic!("{name} has no count"))
                .format()
        };
        assert_eq!(head.changed[Path::new("a.txt")].status, Status::Modified);
        assert_eq!(count("a.txt"), "+1");
        assert_eq!(head.changed[Path::new("b.txt")].status, Status::Modified);
        assert_eq!(count("b.txt"), "+1");
        assert_eq!(
            head.changed[Path::new(".gitignore")].status,
            Status::Deleted
        );
        assert_eq!(count(".gitignore"), "-1");
        assert_eq!(head.changed[Path::new("loose.txt")].status, Status::Added);
        assert_eq!(count("loose.txt"), "+2");
        assert_eq!(head.changed[Path::new("staged.txt")].status, Status::Added);
        assert_eq!(count("staged.txt"), "+1");
        assert!(!head.changed.contains_key(Path::new("debug.log")));
        assert_eq!(head.changed.len(), 5);
    }

    #[test]
    fn a_binary_file_gets_a_status_but_no_count() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), b"\0\0\0binary").expect("write");

        let head = build(dir.path(), now()).expect("build");
        assert_eq!(head.changed[Path::new("a.txt")].status, Status::Modified);
        assert!(head.changed[Path::new("a.txt")].stat.is_none());
    }

    #[test]
    fn the_source_reads_the_committed_content_not_the_working_one() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "one\nchanged\n").expect("write");

        let source = GitSource::open(dir.path()).expect("source");
        assert_eq!(
            source.blob(Path::new("a.txt")),
            Blob::Bytes(b"one\n".to_vec())
        );
        // A file git has never seen has no baseline side at all.
        assert_eq!(source.blob(Path::new("new.txt")), Blob::Absent);
    }

    #[test]
    fn the_source_works_from_a_subdirectory_of_the_repository() {
        let dir = repo();
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("sub/inner.txt"), "deep\n").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "sub"]);

        let source = GitSource::open(&dir.path().join("sub")).expect("source");
        assert_eq!(
            source.blob(Path::new("inner.txt")),
            Blob::Bytes(b"deep\n".to_vec())
        );
        // The root's files are named from the subdirectory, not from here.
        assert_eq!(source.blob(Path::new("a.txt")), Blob::Absent);
    }

    #[test]
    fn a_path_that_cannot_name_an_object_is_refused() {
        let dir = repo();
        let source = GitSource::open(dir.path()).expect("source");
        assert_eq!(source.blob(Path::new("bad\nname.txt")), Blob::Unreadable);
        assert_eq!(source.blob(Path::new("../outside.txt")), Blob::Unreadable);
        assert_eq!(source.blob(Path::new("")), Blob::Unreadable);
    }

    #[test]
    fn the_status_of_some_paths_answers_each_of_them_in_the_order_asked() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "one\nchanged\n").expect("write");
        std::fs::remove_file(dir.path().join("b.txt")).expect("remove");
        std::fs::write(dir.path().join("new.txt"), "new\n").expect("write");
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(7);

        let asked = vec![
            PathBuf::from("new.txt"),
            PathBuf::from("never.txt"),
            PathBuf::from("b.txt"),
            PathBuf::from("a.txt"),
        ];
        let answers = stats(dir.path(), &asked, at);
        let answered: Vec<(&Path, Option<Status>)> = answers
            .iter()
            .map(|(rel, entry)| (rel.as_path(), entry.as_ref().map(|entry| entry.status)))
            .collect();
        assert_eq!(
            answered,
            vec![
                (Path::new("new.txt"), Some(Status::Added)),
                // A path nothing is wrong with is answered, not left out:
                // the answer is what takes it back out of the changed set.
                (Path::new("never.txt"), None),
                (Path::new("b.txt"), Some(Status::Deleted)),
                (Path::new("a.txt"), Some(Status::Modified)),
            ]
        );
        assert_eq!(answers.len(), asked.len());
        assert_eq!(
            answers[0].1.as_ref().map(|entry| entry.changed_at),
            Some(at)
        );
    }

    #[test]
    fn a_list_of_paths_past_the_budget_is_answered_from_the_whole_work_tree() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "one\nchanged\n").expect("write");
        let at = SystemTime::UNIX_EPOCH;

        let mut asked = vec![PathBuf::from("a.txt")];
        for index in 0..60 {
            asked.push(PathBuf::from(format!("{index:04}{}", "x".repeat(160))));
        }
        let budget: usize = asked
            .iter()
            .map(|rel| format!(":(literal){}", rel.display()).len())
            .sum();
        assert!(budget > PATHSPEC_BUDGET, "{budget} fits on one command");

        let answers = stats(dir.path(), &asked, at);
        assert_eq!(answers.len(), asked.len());
        assert_eq!(
            answers[0].1.as_ref().map(|entry| entry.status),
            Some(Status::Modified)
        );
        assert!(
            answers[1..].iter().all(|(_, entry)| entry.is_none()),
            "a path outside the work tree was answered as changed: {answers:?}"
        );
    }

    #[test]
    fn a_git_without_an_answer_leaves_the_caller_to_its_own_state() {
        let dir = tempfile::tempdir().expect("temp dir");
        let at = SystemTime::UNIX_EPOCH;
        // No repository, so nothing could be worked out: the caller is told
        // so by an empty result rather than by paths that all look clean.
        assert!(stats(dir.path(), &[PathBuf::from("a.txt")], at).is_empty());
        assert!(stats(dir.path(), &[], at).is_empty());
    }
}
