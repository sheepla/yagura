//! The contents of the tree as yagura started (§8.3).
//!
//! Outside a repository there is no commit to diff against, so the scan that
//! runs at startup keeps the bytes it finds there instead. What the budget
//! holds is read in the order a coder would want it - source first, small
//! files first - and every file is recorded as seen whether or not its bytes
//! were kept, because that is what tells a file created afterwards (an
//! addition) from one the scan simply could not afford (a modification with
//! no side to count against).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use ignore::WalkBuilder;

use super::{BaselineSource, Blob, Built, ChangedEntry, Kind, Phase, git};
use crate::core::diff::{Diff, DiffStat};
use crate::core::dirload;
use crate::core::fsmodel::Status;
use crate::core::preview::{BINARY_SNIFF_BYTES, MAX_BYTES};

/// How much the whole snapshot may hold (§8.3).
const TOTAL_BUDGET: u64 = 64 * 1024 * 1024;

/// Extensions kept before any other text, so that a budget which runs out
/// halfway still holds the code rather than the prose.
const SOURCE_EXTS: &[&str] = &[
    "rs", "toml", "md", "py", "js", "jsx", "ts", "tsx", "go", "c", "h", "cpp", "hpp", "java", "rb",
    "sh", "bash", "zsh", "json", "yaml", "yml", "html", "css", "sql", "xml", "lua", "kt", "swift",
    "php", "cs", "ex", "exs", "erl", "hs", "ml", "r", "pl", "zig", "nim", "dart",
];

/// What a file looked like when the scan read it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Seen {
    len: u64,
    mtime: Option<SystemTime>,
}

struct Inner {
    /// Every file the scan walked, with or without its bytes.
    seen: BTreeMap<PathBuf, Seen>,
    /// The bytes kept for the files that fitted in both budgets.
    content: BTreeMap<PathBuf, Arc<[u8]>>,
}

/// The startup contents of a tree (§8.3).
#[derive(Clone)]
pub struct Snapshot(Arc<Inner>);

impl Snapshot {
    /// Reads the tree, keeping what the budgets allow (§8.3).
    pub fn capture(root: &Path, ignore_enabled: bool) -> Self {
        Self::capture_with(root, ignore_enabled, MAX_BYTES as u64, TOTAL_BUDGET)
    }

    /// [`Self::capture`] with the budgets written out, for the tests that
    /// cannot afford sixty-four mebibytes of fixtures.
    fn capture_with(
        root: &Path,
        ignore_enabled: bool,
        file_budget: u64,
        total_budget: u64,
    ) -> Self {
        let mut seen = BTreeMap::new();
        let mut candidates = Vec::new();
        for (rel, meta) in walk(root, ignore_enabled) {
            if meta.len <= file_budget {
                candidates.push((priority(&rel), meta.len, rel.clone()));
            }
            seen.insert(rel, meta);
        }
        // Source before other text, and inside each of those the smaller
        // file first, so that a half-spent budget is spent on the files
        // most worth having.
        candidates.sort();
        let mut content = BTreeMap::new();
        let mut total = 0u64;
        for (_, len, rel) in candidates {
            if total + len > total_budget {
                continue;
            }
            let Ok(bytes) = std::fs::read(root.join(&rel)) else {
                continue;
            };
            if !is_text(&bytes) {
                continue;
            }
            total += bytes.len() as u64;
            content.insert(rel, Arc::from(bytes.into_boxed_slice()));
        }
        Snapshot(Arc::new(Inner { seen, content }))
    }

    /// The kept bytes of a file, or `None` when nothing was kept for it.
    fn kept(&self, rel: &Path) -> Option<&Arc<[u8]>> {
        self.0.content.get(rel)
    }
}

impl BaselineSource for Snapshot {
    fn blob(&self, rel: &Path) -> Blob {
        if let Some(bytes) = self.kept(rel) {
            return Blob::Bytes(bytes.to_vec());
        }
        if self.0.seen.contains_key(rel) {
            // Known at startup, but past the budget or not text: there is
            // no side to read, which is what "no baseline" means (§8.3).
            Blob::Unreadable
        } else {
            Blob::Absent
        }
    }
}

/// What differs from `captured` at this moment (§8.3).
///
/// The scan and this walk see the same tree a moment apart, so the common
/// answer is "nothing": every entry below costs a read, and only a file whose
/// size or time moved as far as one.
pub fn build(root: &Path, captured: &Snapshot, now: SystemTime, ignore_enabled: bool) -> Built {
    let current = walk(root, ignore_enabled);
    let mut changed = BTreeMap::new();
    for (rel, after) in &current {
        let Some(before) = captured.0.seen.get(rel) else {
            // The scan never saw it, so it has no baseline side however it
            // came to be there.
            let stat = std::fs::read(root.join(rel))
                .ok()
                .and_then(|bytes| stat_of(&[], &bytes));
            changed.insert(
                rel.clone(),
                ChangedEntry {
                    status: Status::Added,
                    changed_at: now,
                    stat,
                },
            );
            continue;
        };
        if before == after {
            continue;
        }
        let Some(baseline) = captured.kept(rel) else {
            // The bytes were never kept, so it can be named as changed and
            // not counted: the `?` of a file with no baseline (§8.3).
            changed.insert(
                rel.clone(),
                ChangedEntry {
                    status: Status::Modified,
                    changed_at: now,
                    stat: None,
                },
            );
            continue;
        };
        let Ok(bytes) = std::fs::read(root.join(rel)) else {
            continue;
        };
        if bytes.as_slice() == &baseline[..] {
            // Only the timestamp moved; the file is as it was.
            continue;
        }
        changed.insert(
            rel.clone(),
            ChangedEntry {
                status: Status::Modified,
                changed_at: now,
                stat: stat_of(baseline, &bytes),
            },
        );
    }
    // What the scan had and the tree no longer shows was taken away, and
    // the bytes it kept are what its lines are counted against.
    for rel in captured.0.seen.keys() {
        if current.contains_key(rel) {
            continue;
        }
        let stat = captured
            .kept(rel)
            .and_then(|baseline| stat_of(baseline, b""));
        changed.insert(
            rel.clone(),
            ChangedEntry {
                status: Status::Deleted,
                changed_at: now,
                stat,
            },
        );
    }
    Built {
        kind: Kind::Snapshot,
        phase: Phase::Ready,
        repo: git::probe(root).is_ok(),
        head_oid: None,
        changed,
        message: None,
    }
}

/// How the asked paths differ from the snapshot (§7.3).
///
/// This is the same answer `git status` gives for the git baseline: one
/// entry per path, `None` for a path that is as the scan left it.
pub fn stats(
    root: &Path,
    captured: &Snapshot,
    paths: &[PathBuf],
    at: SystemTime,
) -> Vec<(PathBuf, Option<ChangedEntry>)> {
    paths
        .iter()
        .map(|rel| (rel.clone(), answer(root, captured, rel, at)))
        .collect()
}

/// One path's answer, `None` when it is unchanged or says nothing.
fn answer(root: &Path, captured: &Snapshot, rel: &Path, at: SystemTime) -> Option<ChangedEntry> {
    let path = root.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        // Gone: a deletion only when the scan had it to begin with.
        if !captured.0.seen.contains_key(rel) {
            return None;
        }
        let stat = captured
            .kept(rel)
            .and_then(|baseline| stat_of(baseline, b""));
        return Some(ChangedEntry {
            status: Status::Deleted,
            changed_at: at,
            stat,
        });
    };
    if !meta.is_file() {
        // Directories are not content and links are not followed by the
        // scan, so neither has a side to compare.
        return None;
    }
    let Some(before) = captured.0.seen.get(rel) else {
        // The scan never saw it: an addition, counted if it is text.
        let stat = std::fs::read(&path)
            .ok()
            .and_then(|bytes| stat_of(&[], &bytes));
        return Some(ChangedEntry {
            status: Status::Added,
            changed_at: at,
            stat,
        });
    };
    if before.len == meta.len() && before.mtime == meta.modified().ok() {
        return None;
    }
    let Some(baseline) = captured.kept(rel) else {
        return Some(ChangedEntry {
            status: Status::Modified,
            changed_at: at,
            stat: None,
        });
    };
    let bytes = std::fs::read(&path).ok()?;
    if bytes.as_slice() == &baseline[..] {
        return None;
    }
    Some(ChangedEntry {
        status: Status::Modified,
        changed_at: at,
        stat: stat_of(baseline, &bytes),
    })
}

/// The `+N -N ~N` of two versions of a file, `None` when either side is
/// not text or the two cannot be counted (§10).
fn stat_of(baseline: &[u8], current: &[u8]) -> Option<DiffStat> {
    if !is_text(baseline) || !is_text(current) {
        return None;
    }
    let old = std::str::from_utf8(baseline).ok()?;
    let new = std::str::from_utf8(current).ok()?;
    Diff::compute(old, new).ok().map(|diff| diff.stat)
}

/// True for bytes the preview would still call text: no NUL in the window
/// it sniffs (§9.1).
fn is_text(bytes: &[u8]) -> bool {
    let window = bytes.len().min(BINARY_SNIFF_BYTES);
    !bytes[..window].contains(&0)
}

/// 0 for a source file, 1 for anything else: the order the budget prefers.
fn priority(rel: &Path) -> u8 {
    let extension = rel
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    u8::from(!SOURCE_EXTS.contains(&extension.as_str()))
}

/// Every plain file below `root` that the tree would show, with what it
/// weighs and when it was last written.
fn walk(root: &Path, ignore_enabled: bool) -> BTreeMap<PathBuf, Seen> {
    let mut files = BTreeMap::new();
    let mut builder = WalkBuilder::new(root);
    builder
        .follow_links(false)
        .standard_filters(false)
        .hidden(false)
        .parents(ignore_enabled)
        .ignore(ignore_enabled)
        .git_ignore(ignore_enabled)
        .git_global(ignore_enabled)
        .git_exclude(ignore_enabled)
        // Ignore files are honoured outside a repository too: a plain
        // directory has them, and the tree already applies them (§7.4).
        .require_git(false)
        // `.git` and the editor's leftovers are not part of what is watched,
        // so the walk never descends into them either.
        .filter_entry(|entry| !dirload::is_skipped(entry.file_name()));
    for result in builder.build() {
        let Ok(entry) = result else {
            continue;
        };
        if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        files.insert(
            rel.to_path_buf(),
            Seen {
                len: metadata.len(),
                mtime: metadata.modified().ok(),
            },
        );
    }
    files
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, body).expect("write");
    }

    fn status_of(built: &Built, rel: &str) -> Option<Status> {
        built.changed.get(Path::new(rel)).map(|entry| entry.status)
    }

    fn counts_of(built: &Built, rel: &str) -> Option<String> {
        built
            .changed
            .get(Path::new(rel))
            .and_then(|entry| entry.stat)
            .map(|stat| stat.format())
    }

    #[test]
    fn the_scan_keeps_the_bytes_it_read_before_the_file_changed() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), "main.rs", "fn main() {}\n");
        let captured = Snapshot::capture(tmp.path(), true);

        write(tmp.path(), "main.rs", "fn main() {}\nfn other() {}\n");

        assert_eq!(
            captured.blob(Path::new("main.rs")),
            Blob::Bytes(b"fn main() {}\n".to_vec()),
            "the snapshot is of the start, not of now"
        );
        let built = build(tmp.path(), &captured, SystemTime::UNIX_EPOCH, true);
        assert_eq!(
            status_of(&built, "main.rs"),
            Some(Status::Modified),
            "{:?}",
            built.changed
        );
        assert_eq!(counts_of(&built, "main.rs").as_deref(), Some("+1"));
    }

    #[test]
    fn an_untouched_tree_reports_nothing_changed() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), "readme.md", "# yagura\n");
        write(tmp.path(), "src/lib.rs", "pub fn x() {}\n");
        let captured = Snapshot::capture(tmp.path(), true);

        let built = build(tmp.path(), &captured, SystemTime::UNIX_EPOCH, true);

        assert!(
            built.changed.is_empty(),
            "a tree the scan just read is as the scan left it: {:?}",
            built.changed
        );
        assert_eq!(built.phase, Phase::Ready);
    }

    #[test]
    fn a_file_created_after_the_scan_is_an_addition() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), "old.rs", "one\n");
        let captured = Snapshot::capture(tmp.path(), true);

        write(tmp.path(), "new.rs", "one\ntwo\n");
        let built = build(tmp.path(), &captured, SystemTime::UNIX_EPOCH, true);

        assert_eq!(status_of(&built, "new.rs"), Some(Status::Added));
        assert_eq!(counts_of(&built, "new.rs").as_deref(), Some("+2"));
        assert_eq!(
            captured.blob(Path::new("new.rs")),
            Blob::Absent,
            "a file that was not there has no baseline side"
        );
    }

    #[test]
    fn a_file_removed_after_the_scan_is_a_deletion() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), "gone.rs", "one\ntwo\n");
        let captured = Snapshot::capture(tmp.path(), true);

        fs::remove_file(tmp.path().join("gone.rs")).expect("remove");
        let built = build(tmp.path(), &captured, SystemTime::UNIX_EPOCH, true);

        assert_eq!(status_of(&built, "gone.rs"), Some(Status::Deleted));
        assert_eq!(counts_of(&built, "gone.rs").as_deref(), Some("-2"));
        assert_eq!(
            captured.blob(Path::new("gone.rs")),
            Blob::Bytes(b"one\ntwo\n".to_vec()),
            "a deleted file is read from the snapshot"
        );
    }

    #[test]
    fn a_file_only_the_timestamp_moved_is_unchanged() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), "same.rs", "one\n");
        let captured = Snapshot::capture(tmp.path(), true);

        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        let file = tmp.path().join("same.rs");
        fs::File::options()
            .append(true)
            .open(&file)
            .expect("open")
            .set_modified(later)
            .expect("touch");

        let built = build(tmp.path(), &captured, SystemTime::UNIX_EPOCH, true);
        assert!(built.changed.is_empty(), "{:?}", built.changed);
    }

    #[test]
    fn a_binary_file_changes_without_a_count() {
        let tmp = tempfile::tempdir().expect("temp dir");
        fs::write(tmp.path().join("logo.png"), [0x89, b'P', 0, 1]).expect("write");
        let captured = Snapshot::capture(tmp.path(), true);

        fs::write(tmp.path().join("logo.png"), [0x89, b'P', 0, 1, 2]).expect("rewrite");

        assert_eq!(
            captured.blob(Path::new("logo.png")),
            Blob::Unreadable,
            "binary bytes are seen but never kept"
        );
        let built = build(tmp.path(), &captured, SystemTime::UNIX_EPOCH, true);
        assert_eq!(status_of(&built, "logo.png"), Some(Status::Modified));
        assert_eq!(
            counts_of(&built, "logo.png"),
            None,
            "there is no side to count"
        );
    }

    #[test]
    fn the_per_file_budget_keeps_the_small_and_sees_the_large() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), "small.rs", "small\n");
        let large = "x".repeat(64);
        write(tmp.path(), "large.rs", &large);

        let captured = Snapshot::capture_with(tmp.path(), true, 32, u64::MAX);

        assert_eq!(
            captured.blob(Path::new("small.rs")),
            Blob::Bytes(b"small\n".to_vec())
        );
        assert_eq!(
            captured.blob(Path::new("large.rs")),
            Blob::Unreadable,
            "over budget, but still a file the scan knows"
        );
        // Knowing it is what makes an edit to it a modification rather
        // than an addition.
        write(tmp.path(), "large.rs", "edited");
        let built = build(tmp.path(), &captured, SystemTime::UNIX_EPOCH, true);
        assert_eq!(status_of(&built, "large.rs"), Some(Status::Modified));
        assert_eq!(counts_of(&built, "large.rs"), None);
    }

    #[test]
    fn the_total_budget_goes_to_source_files_first() {
        let tmp = tempfile::tempdir().expect("temp dir");
        // Four files of eight bytes, room for three of them.
        write(tmp.path(), "notes.txt", "12345678");
        write(tmp.path(), "big.txt", "123456789");
        write(tmp.path(), "lib.rs", "12345678");
        write(tmp.path(), "main.rs", "123456789");

        let captured = Snapshot::capture_with(tmp.path(), true, u64::MAX, 25);

        for kept in ["notes.txt", "lib.rs", "main.rs"] {
            assert!(
                matches!(captured.blob(Path::new(kept)), Blob::Bytes(_)),
                "{kept} was dropped before a file it outranks"
            );
        }
        assert_eq!(
            captured.blob(Path::new("big.txt")),
            Blob::Unreadable,
            "the longest of the texts is what the budget gives up"
        );
    }

    #[test]
    fn the_scan_leaves_ignored_files_alone() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), ".gitignore", "*.log\n");
        write(tmp.path(), "keep.rs", "fn keep() {}\n");
        write(tmp.path(), "noise.log", "noise\n");

        let captured = Snapshot::capture(tmp.path(), true);

        assert_eq!(
            captured.blob(Path::new("keep.rs")),
            Blob::Bytes(b"fn keep() {}\n".to_vec())
        );
        assert_eq!(
            captured.blob(Path::new("noise.log")),
            Blob::Absent,
            "an ignored file is not part of the tree and not part of the baseline"
        );
        assert_eq!(
            captured.blob(Path::new(".gitignore")),
            Blob::Bytes(b"*.log\n".to_vec())
        );
    }

    #[test]
    fn stats_answers_the_paths_it_is_asked_about() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), "kept.rs", "one\ntwo\n");
        write(tmp.path(), "still.rs", "same\n");
        let captured = Snapshot::capture(tmp.path(), true);
        let at = SystemTime::UNIX_EPOCH;

        write(tmp.path(), "kept.rs", "one\ntwo\nthree\n");
        write(tmp.path(), "new.rs", "brand\n");
        fs::remove_file(tmp.path().join("still.rs")).expect("remove");

        let answers: BTreeMap<PathBuf, Option<ChangedEntry>> = stats(
            tmp.path(),
            &captured,
            &[
                PathBuf::from("kept.rs"),
                PathBuf::from("still.rs"),
                PathBuf::from("new.rs"),
                PathBuf::from("missing.rs"),
            ],
            at,
        )
        .into_iter()
        .collect();

        let kept = answers[&PathBuf::from("kept.rs")].expect("kept changed");
        assert_eq!(kept.status, Status::Modified);
        assert_eq!(kept.stat.expect("counted").format(), "+1");
        assert_eq!(
            answers[&PathBuf::from("still.rs")].expect("deleted").status,
            Status::Deleted
        );
        let added = answers[&PathBuf::from("new.rs")].expect("added");
        assert_eq!(added.status, Status::Added);
        assert_eq!(added.stat.expect("counted").format(), "+1");
        assert!(
            answers[&PathBuf::from("missing.rs")].is_none(),
            "a path the scan never had says nothing"
        );
    }

    #[test]
    fn a_path_that_has_not_moved_says_it_is_unchanged() {
        let tmp = tempfile::tempdir().expect("temp dir");
        write(tmp.path(), "kept.rs", "one\n");
        let captured = Snapshot::capture(tmp.path(), true);

        let answers = stats(
            tmp.path(),
            &captured,
            &[PathBuf::from("kept.rs")],
            SystemTime::UNIX_EPOCH,
        );

        assert_eq!(answers, vec![(PathBuf::from("kept.rs"), None)]);
    }
}
