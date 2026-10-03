//! Running effects outside the UI loop.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::mpsc::Sender;

use crate::app::action::{Action, SCAN_CHUNK, StatAnswer};
use crate::app::effect::Effect;
use crate::core::baseline::{self, BaselineSource, Blob, Kind, Phase};
use crate::core::fswatch::watcher::FsWatcher;
use crate::core::preview::Baseline;
use crate::core::{dirload, preview};

/// How often the clock tick fires.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// How long the repository has to go quiet before `HEAD` is read (§8.2).
const HEAD_DEBOUNCE: Duration = Duration::from_millis(300);

/// How far apart two moves of `HEAD` are kept, so that a burst of commits
/// costs one rebuild rather than one per event (§8.2).
const HEAD_THROTTLE: Duration = Duration::from_millis(500);

/// One `HEAD` check at a time, with the requests that arrived meanwhile.
#[derive(Default)]
struct HeadState {
    /// A request is waiting to be answered.
    pending: bool,
    /// A check is under way.
    running: bool,
    /// What `HEAD` was last read as, and when that answer was sent. Both
    /// belong to the check and not to one run of it: a check that starts
    /// later has to keep the same spacing as the one it follows.
    previous: Option<Option<String>>,
    sent_at: Option<Instant>,
}

/// Takes the lock on `state`, a lock that a panic has left behind being no
/// reason to stop reading the tree.
fn lock(state: &Arc<Mutex<HeadState>>) -> std::sync::MutexGuard<'_, HeadState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The source of baseline content of the newest build, with its build number.
type BaselineSlot = Arc<Mutex<Option<(u64, Arc<dyn BaselineSource>)>>>;

/// The startup scan's answer, once it has one.
type SnapshotSlot = Arc<Mutex<Option<Arc<crate::core::baseline::snapshot::Snapshot>>>>;

/// How long a build waits for the startup scan before giving up on it.
const SNAPSHOT_WAIT: Duration = Duration::from_secs(30);

/// The snapshot out of the slot, waiting for the scan to hand it over.
///
/// The wait happens on a blocking worker, never on the UI thread (invariant
/// I1); a scan of a whole tree is a few tens of mebibytes at the outside.
fn wait_for_snapshot(
    slot: &SnapshotSlot,
) -> Option<Arc<crate::core::baseline::snapshot::Snapshot>> {
    let deadline = Instant::now() + SNAPSHOT_WAIT;
    loop {
        if let Some(found) = slot.lock().ok().and_then(|guard| guard.clone()) {
            return Some(found);
        }
        if Instant::now() >= deadline {
            tracing::warn!("the startup scan did not finish in time");
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Walks the whole tree for the file finder, answering in chunks (§13.2).
///
/// The walk is parallel because a repository is a tree and the list wants its
/// first rows long before the last ones are read, and it answers in chunks
/// because one action per file would fill the channel instead of the list.
/// What a walk sees is what it sees: the paths come out relative to the root,
/// the way every other path in the application is held.
fn scan_files(root: &Path, ignore_enabled: bool, generation: u64, tx: &Sender<Action>) {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .follow_links(false)
        .standard_filters(false)
        .hidden(false)
        .parents(ignore_enabled)
        .ignore(ignore_enabled)
        .git_ignore(ignore_enabled)
        .git_global(ignore_enabled)
        .git_exclude(ignore_enabled)
        // Honour ignore files outside a repository too, the way the tree
        // lists directories (§7.4).
        .require_git(false)
        .filter_entry(|entry| !dirload::is_skipped(entry.file_name()));
    builder.build_parallel().run(|| {
        let root = root.to_path_buf();
        let tx = tx.clone();
        let mut chunk: Vec<PathBuf> = Vec::with_capacity(SCAN_CHUNK);
        Box::new(move |result| {
            let Ok(entry) = result else {
                return ignore::WalkState::Continue;
            };
            if entry.file_type().is_some_and(|kind| kind.is_file())
                && let Ok(rel) = entry.path().strip_prefix(&root)
            {
                chunk.push(rel.to_path_buf());
                if chunk.len() >= SCAN_CHUNK {
                    let rels = std::mem::take(&mut chunk);
                    let action = Action::FilesFound { generation, rels };
                    if tx.blocking_send(action).is_err() {
                        // The store is gone: nobody is reading the tree.
                        return ignore::WalkState::Quit;
                    }
                    chunk = Vec::with_capacity(SCAN_CHUNK);
                }
            }
            ignore::WalkState::Continue
        })
    });
    if tx
        .blocking_send(Action::FilesScanDone { generation })
        .is_err()
    {
        tracing::debug!("store is gone, dropping the end of the walk");
    }
}

/// Starts effects and returns their results as actions.
pub struct EffectRunner {
    root: PathBuf,
    ignore_enabled: bool,
    action_tx: Sender<Action>,
    baseline: BaselineSlot,
    /// The watch on the root, started by the first [`Effect::WatchDirs`] and
    /// kept alive for as long as the application runs.
    watcher: Mutex<Option<FsWatcher>>,
    /// Whether a check of `HEAD` is wanted, and whether one is running.
    head: Arc<Mutex<HeadState>>,
    /// What the startup scan read, started once by the first build.
    snapshot: SnapshotSlot,
    /// Whether the scan has been started, so that it runs exactly once.
    snapshot_started: AtomicBool,
}

impl EffectRunner {
    pub fn new(root: PathBuf, ignore_enabled: bool, action_tx: Sender<Action>) -> Self {
        Self {
            root,
            ignore_enabled,
            action_tx,
            baseline: Arc::new(Mutex::new(None)),
            watcher: Mutex::new(None),
            head: Arc::new(Mutex::new(HeadState::default())),
            snapshot: Arc::new(Mutex::new(None)),
            snapshot_started: AtomicBool::new(false),
        }
    }

    /// Starts the scan of the tree as it stands (§8.3), once.
    ///
    /// It runs from the first build of any kind, so that a snapshot needed
    /// later is of the start of the session rather than of the moment it was
    /// asked for.
    fn start_snapshot(&self) {
        if self.snapshot_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let root = self.root.clone();
        let ignore_enabled = self.ignore_enabled;
        let slot = Arc::clone(&self.snapshot);
        tokio::task::spawn_blocking(move || {
            let captured = Arc::new(crate::core::baseline::snapshot::Snapshot::capture(
                &root,
                ignore_enabled,
            ));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(captured);
        });
    }

    /// Runs one effect.
    ///
    /// Nothing here runs on the UI thread: reads happen on a blocking worker
    /// and the clock lives in its own task (invariant I1).
    pub fn run(&self, effect: Effect) {
        match effect {
            Effect::LoadDir {
                node,
                generation,
                rel,
            } => {
                let dir = self.root.join(&rel);
                let ignore_enabled = self.ignore_enabled;
                let tx = self.action_tx.clone();
                tokio::task::spawn_blocking(move || {
                    let result = dirload::load_dir(&dir, ignore_enabled);
                    let action = Action::DirLoaded {
                        node,
                        generation,
                        result,
                    };
                    // blocking_send is allowed here: this is a blocking worker.
                    if let Err(err) = tx.blocking_send(action) {
                        tracing::debug!(%err, "store is gone, dropping listing");
                    }
                });
            }
            Effect::LoadPreview {
                rel,
                generation,
                mode,
                baseline_ver,
            } => {
                let path = self.root.join(&rel);
                let tx = self.action_tx.clone();
                let slot = Arc::clone(&self.baseline);
                tokio::task::spawn_blocking(move || {
                    // A content view of a file that still exists needs no
                    // baseline; a diff, or a file that is gone, does: the
                    // baseline is also what a deleted file is read from (§6).
                    let present = std::fs::symlink_metadata(&path).is_ok();
                    let want_diff = mode.is_diff();
                    let baseline =
                        resolve_baseline(&slot, baseline_ver, &rel, want_diff || !present);
                    let result = preview::load_doc_with(&path, &rel, baseline, want_diff);
                    let action = Action::PreviewLoaded {
                        rel,
                        generation,
                        result,
                    };
                    if let Err(err) = tx.blocking_send(action) {
                        tracing::debug!(%err, "store is gone, dropping preview");
                    }
                });
            }
            Effect::BuildBaseline { kind, ver } => {
                // Every build starts the startup scan, so that a snapshot is
                // of the beginning of the session even when the first build
                // was the git one (§8.3).
                self.start_snapshot();
                let root = self.root.clone();
                let tx = self.action_tx.clone();
                let slot = Arc::clone(&self.baseline);
                let snapshot_slot = Arc::clone(&self.snapshot);
                let ignore_enabled = self.ignore_enabled;
                tokio::task::spawn_blocking(move || {
                    // Only a snapshot build has anything to read from the
                    // scan, and it waits here rather than on the UI thread.
                    let captured = (kind == Kind::Snapshot)
                        .then(|| wait_for_snapshot(&snapshot_slot))
                        .flatten();
                    let built = baseline::build(
                        &root,
                        kind,
                        SystemTime::now(),
                        captured.clone(),
                        ignore_enabled,
                    );
                    // The source has to answer before the reducer can ask for
                    // a diff, so it is stored under this build number before
                    // the result goes out. A build that already moved on keeps
                    // the source it stored.
                    if built.phase == Phase::Ready
                        && let Some(source) = source_for(&root, &built, captured)
                        && let Ok(mut slot) = slot.lock()
                        && slot.as_ref().is_none_or(|(stored, _)| *stored <= ver)
                    {
                        *slot = Some((ver, source));
                    }
                    let action = Action::BaselineReady { ver, built };
                    if let Err(err) = tx.blocking_send(action) {
                        tracing::debug!(%err, "store is gone, dropping the baseline");
                    }
                });
            }
            Effect::ScheduleTick => self.spawn_clock(),
            Effect::WatchDirs { add, remove } => self.watch(&add, &remove),
            Effect::ComputeStats { kind, at, paths } => self.compute_stats(kind, at, &paths),
            Effect::CheckHead => self.check_head(),
            Effect::ScanFiles { generation } => {
                let root = self.root.clone();
                let ignore_enabled = self.ignore_enabled;
                let tx = self.action_tx.clone();
                tokio::task::spawn_blocking(move || {
                    scan_files(&root, ignore_enabled, generation, &tx);
                });
            }
        }
    }

    /// Answers how some paths differ from the baseline (§7.3).
    ///
    /// The question is asked off the UI thread like every other read
    /// (invariant I1), and only for the paths the reducer named: a batch of
    /// changes costs one `git status` of those paths rather than one of the
    /// whole work tree. Outside a repository there is no command to run -
    /// the startup scan already holds the side to compare against (§8.3).
    fn compute_stats(&self, kind: Kind, at: SystemTime, paths: &[(PathBuf, u64)]) {
        let root = self.root.clone();
        let tx = self.action_tx.clone();
        let asked = paths.to_vec();
        let snapshot_slot = Arc::clone(&self.snapshot);
        tokio::task::spawn_blocking(move || {
            let rels: Vec<PathBuf> = asked.iter().map(|(rel, _)| rel.clone()).collect();
            let answers = match kind {
                Kind::Head => baseline::git::stats(&root, &rels, at),
                Kind::Snapshot => match wait_for_snapshot(&snapshot_slot) {
                    Some(captured) => baseline::snapshot::stats(&root, &captured, &rels, at),
                    None => return,
                },
            };
            if answers.is_empty() {
                // Nothing could be worked out: the state it has is better
                // than an answer nobody can check.
                return;
            }
            let results = answers
                .into_iter()
                .zip(asked.iter())
                .map(|((rel, entry), &(_, generation))| StatAnswer {
                    rel,
                    generation,
                    entry,
                })
                .collect();
            let action = Action::StatsComputed { results };
            if let Err(err) = tx.blocking_send(action) {
                tracing::debug!(%err, "store is gone, dropping stats");
            }
        });
    }

    /// Reads `HEAD` once the repository has gone quiet (§8.2).
    ///
    /// A request that arrives while a check runs is answered by the run in
    /// progress rather than starting a second one, and only a value that
    /// really moved is sent on: a repository that merely wrote its index
    /// costs nothing but the read.
    fn check_head(&self) {
        {
            let mut head = lock(&self.head);
            head.pending = true;
            if head.running {
                return;
            }
            head.running = true;
        }
        let head = Arc::clone(&self.head);
        let tx = self.action_tx.clone();
        let root = self.root.clone();
        tokio::spawn(async move {
            loop {
                // Everything from here on is read by this turn: a request
                // that arrives later is the next turn's.
                lock(&head).pending = false;
                tokio::time::sleep(HEAD_DEBOUNCE).await;
                let reading = root.clone();
                let Ok(oid) =
                    tokio::task::spawn_blocking(move || baseline::git::head_oid(&reading)).await
                else {
                    // The read itself failed, which is not a reason for
                    // every later request to go unanswered.
                    let mut state = lock(&head);
                    state.running = false;
                    state.pending = false;
                    break;
                };
                let (moved, wait) = {
                    let state = lock(&head);
                    let moved = state
                        .previous
                        .as_ref()
                        .is_none_or(|known| known.as_ref() != oid.as_ref());
                    let wait = state.sent_at.map_or(Duration::ZERO, |sent| {
                        HEAD_THROTTLE.saturating_sub(sent.elapsed())
                    });
                    (moved, wait)
                };
                if moved {
                    // The rebuild of one move is under way while the next
                    // move is being read, so the two are spaced out (§8.2).
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                    let action = Action::HeadChecked { oid: oid.clone() };
                    if tx.send(action).await.is_err() {
                        // The store is gone: nobody is reading the tree.
                        let mut state = lock(&head);
                        state.running = false;
                        state.pending = false;
                        break;
                    }
                    lock(&head).sent_at = Some(Instant::now());
                }
                lock(&head).previous = Some(oid);
                // Another request waits for this run rather than starting
                // its own; otherwise the run is over and the flag is free.
                let again = {
                    let mut state = lock(&head);
                    if state.pending {
                        true
                    } else {
                        state.running = false;
                        false
                    }
                };
                if !again {
                    break;
                }
            }
        });
    }

    /// Applies the watch changes a listing asked for.
    ///
    /// The watch is started the first time it is needed rather than at
    /// startup, so an application that never asks for one pays nothing for
    /// it. A watch that cannot be started leaves the tree to the listings
    /// alone: it is a loss of freshness, not a reason to stop.
    fn watch(&self, add: &[PathBuf], remove: &[PathBuf]) {
        let mut slot = self.watcher.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            match FsWatcher::start(self.root.clone(), self.action_tx.clone()) {
                Ok(watcher) => *slot = Some(watcher),
                Err(err) => {
                    tracing::warn!(%err, root = %self.root.display(), "cannot watch the tree");
                    return;
                }
            }
        }
        let Some(watcher) = slot.as_mut() else {
            return;
        };
        for rel in remove {
            watcher.unwatch(rel);
        }
        for rel in add {
            if let Err(err) = watcher.watch(rel) {
                tracing::debug!(%err, rel = %rel.display(), "cannot watch a directory");
            }
        }
    }

    fn spawn_clock(&self) {
        let tx = self.action_tx.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval_at(
                tokio::time::Instant::now() + TICK_INTERVAL,
                TICK_INTERVAL,
            );
            loop {
                ticker.tick().await;
                if tx
                    .send(Action::Tick {
                        now: SystemTime::now(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
    }
}

/// The baseline to diff against, or `None` for the plain content on disk.
///
/// A diff mode always answers with a baseline: when the build the request
/// names is not the one the runner holds, the answer is "no baseline" rather
/// than content from a baseline that has already been replaced. The reader
/// of a content view asks for the same answer only when the file itself is
/// gone, and then the baseline is its content (§6).
fn resolve_baseline(
    slot: &BaselineSlot,
    baseline_ver: u64,
    rel: &Path,
    ask: bool,
) -> Option<Baseline> {
    if !ask {
        return None;
    }
    let source = slot.lock().ok().and_then(|guard| {
        let (stored, source) = guard.as_ref()?;
        (*stored == baseline_ver).then(|| Arc::clone(source))
    });
    Some(match source {
        None => Baseline::Unavailable,
        Some(source) => match source.blob(rel) {
            Blob::Bytes(bytes) => Baseline::Bytes(bytes),
            Blob::Absent => Baseline::Absent,
            Blob::Unreadable => Baseline::Unavailable,
        },
    })
}

/// A content reader for a finished git baseline, when there is a `HEAD`.
fn git_source(root: &Path, built: &baseline::Built) -> Option<Arc<dyn BaselineSource>> {
    // On an unborn branch `HEAD:path` never resolves, so there is no commit
    // to read and no source worth starting.
    built.head_oid.as_ref()?;
    match baseline::git::GitSource::open(root) {
        Ok(source) => Some(Arc::new(source)),
        Err(err) => {
            tracing::debug!(%err, "no baseline source");
            None
        }
    }
}

/// The content reader for a build that finished: git's when there is a
/// `HEAD` to read through, the startup scan's own bytes otherwise (§8.3).
fn source_for(
    root: &Path,
    built: &baseline::Built,
    captured: Option<Arc<crate::core::baseline::snapshot::Snapshot>>,
) -> Option<Arc<dyn BaselineSource>> {
    match built.kind {
        Kind::Head => git_source(root, built),
        Kind::Snapshot => captured.map(|captured| captured as Arc<dyn BaselineSource>),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::app::state::PreviewMode;
    use crate::core::fsmodel::{FsModel, NodeKind};

    #[tokio::test]
    async fn loading_a_directory_sends_the_listing_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("a.txt"), "x").expect("write");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(dir.path().to_path_buf(), true, tx);
        let model = FsModel::new(dir.path().to_path_buf());
        let root = model.root_id();
        runner.run(Effect::LoadDir {
            node: root,
            generation: 7,
            rel: PathBuf::new(),
        });
        let action = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no listing in time")
            .expect("channel closed");
        let Action::DirLoaded {
            node,
            generation,
            result,
        } = action
        else {
            panic!("expected a directory listing");
        };
        assert_eq!(node, root);
        assert_eq!(generation, 7);
        let entries = result.expect("listing");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, std::ffi::OsString::from("a.txt"));
        assert_eq!(entries[0].kind, NodeKind::File);
    }

    #[tokio::test]
    async fn a_failed_listing_is_reported_as_an_error() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(PathBuf::from("/definitely/not/here"), true, tx);
        let model = FsModel::new(PathBuf::from("/definitely/not/here"));
        let root = model.root_id();
        runner.run(Effect::LoadDir {
            node: root,
            generation: 1,
            rel: PathBuf::new(),
        });
        let action = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no answer in time")
            .expect("channel closed");
        let Action::DirLoaded { result, .. } = action else {
            panic!("expected a directory listing");
        };
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn the_clock_ticks_about_once_a_second() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(PathBuf::from("/root"), true, tx);
        runner.run(Effect::ScheduleTick);
        let action = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("no tick in time")
            .expect("channel closed");
        assert!(matches!(action, Action::Tick { .. }));
    }

    #[tokio::test]
    async fn building_a_baseline_sends_its_result_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(dir.path().to_path_buf(), true, tx);
        // If the temp directory happens to sit inside a repository the
        // assertions on the content would not hold, so the round trip is all
        // that is checked then.
        let inside_repo = crate::core::baseline::git::probe(dir.path()).is_ok();
        runner.run(Effect::BuildBaseline {
            kind: crate::core::baseline::Kind::Head,
            ver: 3,
        });
        let action = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("no baseline in time")
            .expect("channel closed");
        let Action::BaselineReady { ver, built } = action else {
            panic!("expected a baseline result");
        };
        assert_eq!(ver, 3);
        assert_eq!(built.kind, crate::core::baseline::Kind::Head);
        if !inside_repo {
            assert_eq!(built.phase, crate::core::baseline::Phase::Unavailable);
            assert!(!built.repo);
            assert!(built.message.is_some());
        }
    }

    /// Waits for the startup scan to hand over what it read (§8.3).
    async fn scanned(runner: &EffectRunner) {
        let wait = async {
            loop {
                let done = runner
                    .snapshot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_some();
                if done {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(30), wait)
            .await
            .expect("the startup scan never finished");
    }

    #[tokio::test]
    async fn a_snapshot_baseline_holds_the_contents_the_session_started_with() {
        use crate::core::fsmodel::Status;

        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").expect("write");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(dir.path().to_path_buf(), true, tx);

        // The first build of any kind starts the scan, and a later build
        // waits for it rather than racing it.
        runner.run(Effect::BuildBaseline {
            kind: Kind::Head,
            ver: 1,
        });
        scanned(&runner).await;
        let git_answer = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("no baseline in time")
            .expect("channel closed");
        assert!(matches!(git_answer, Action::BaselineReady { ver: 1, .. }));

        std::fs::write(dir.path().join("main.rs"), "fn main() { run(); }\n").expect("write");

        runner.run(Effect::BuildBaseline {
            kind: Kind::Snapshot,
            ver: 2,
        });
        let action = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("no snapshot in time")
            .expect("channel closed");
        let Action::BaselineReady { ver, built } = action else {
            panic!("expected a baseline result");
        };
        assert_eq!(ver, 2);
        assert_eq!(built.kind, Kind::Snapshot);
        assert_eq!(built.phase, Phase::Ready);
        let entry = built
            .changed
            .get(Path::new("main.rs"))
            .expect("the edit is a change against the start");
        assert_eq!(entry.status, Status::Modified);
        assert_eq!(entry.stat.expect("counted").format(), "~1");
    }

    #[tokio::test]
    async fn stats_for_a_snapshot_compare_with_the_scan_and_not_with_git() {
        use crate::core::fsmodel::Status;

        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("kept.rs"), "one\n").expect("write");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(dir.path().to_path_buf(), true, tx);
        runner.run(Effect::BuildBaseline {
            kind: Kind::Head,
            ver: 1,
        });
        scanned(&runner).await;
        let _ = rx.recv().await.expect("channel closed");

        std::fs::write(dir.path().join("kept.rs"), "one\ntwo\n").expect("write");
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        runner.run(Effect::ComputeStats {
            kind: Kind::Snapshot,
            at,
            paths: vec![(PathBuf::from("kept.rs"), 4)],
        });
        let action = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("no stats in time")
            .expect("channel closed");
        let Action::StatsComputed { results } = action else {
            panic!("expected a stats result");
        };
        assert_eq!(results.len(), 1, "{results:?}");
        let answer = &results[0];
        assert_eq!(answer.generation, 4);
        assert_eq!(answer.rel, PathBuf::from("kept.rs"));
        let entry = answer.entry.expect("the file moved since the scan");
        assert_eq!(entry.status, Status::Modified);
        assert_eq!(entry.stat.expect("counted").format(), "+1");
        assert_eq!(entry.changed_at, at);
    }

    #[tokio::test]
    async fn a_result_is_not_sent_after_the_store_is_gone() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let runner = EffectRunner::new(PathBuf::from("/root"), true, tx);
        drop(rx);
        let dir = tempfile::tempdir().expect("temp dir");
        runner.run(Effect::LoadDir {
            node: FsModel::new(dir.path().to_path_buf()).root_id(),
            generation: 1,
            rel: PathBuf::new(),
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
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

    /// Builds a repository with one commit, then changes the file, so that
    /// the working copy differs from the baseline.
    fn changed_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        git(dir.path(), &["init"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "initial"]);
        std::fs::write(dir.path().join("a.txt"), "one\nchanged\n").expect("write");
        dir
    }

    #[tokio::test]
    async fn a_diff_load_reads_the_baseline_the_build_holds() {
        let dir = changed_repo();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(dir.path().to_path_buf(), true, tx);

        runner.run(Effect::BuildBaseline {
            kind: Kind::Head,
            ver: 1,
        });
        let action = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("no baseline in time")
            .expect("channel closed");
        let Action::BaselineReady { ver, built } = action else {
            panic!("expected a baseline result");
        };
        assert_eq!(ver, 1);
        if built.phase != Phase::Ready {
            // Without a repository there is no source to hold, and the rest
            // of the round trip is covered by the reducer tests.
            return;
        }

        // The changed file diffs against the committed content.
        runner.run(Effect::LoadPreview {
            rel: PathBuf::from("a.txt"),
            generation: 1,
            mode: PreviewMode::InlineDiff,
            baseline_ver: 1,
        });
        let doc = preview_loaded(&mut rx).await;
        let diff = doc.diff.expect("the changed file must have a diff");
        assert_eq!(diff.stat.added, 1);
        assert_eq!(doc.lines, vec!["one", "changed"]);

        // A build the runner never stored answers with no baseline rather
        // than with content from a build that is gone.
        runner.run(Effect::LoadPreview {
            rel: PathBuf::from("a.txt"),
            generation: 2,
            mode: PreviewMode::InlineDiff,
            baseline_ver: 99,
        });
        let doc = preview_loaded(&mut rx).await;
        assert!(doc.diff.is_none());
        assert_eq!(
            doc.diff_note,
            Some(crate::core::preview::DiffNote::NoBaseline)
        );
        assert_eq!(doc.lines, vec!["one", "changed"], "the content stays");

        // The content mode never asks for one.
        runner.run(Effect::LoadPreview {
            rel: PathBuf::from("a.txt"),
            generation: 3,
            mode: PreviewMode::Content,
            baseline_ver: 1,
        });
        let doc = preview_loaded(&mut rx).await;
        assert!(doc.diff.is_none());
        assert!(doc.diff_note.is_none());
        assert_eq!(doc.lines, vec!["one", "changed"]);
    }

    #[tokio::test]
    async fn a_burst_of_head_requests_is_answered_by_one_read() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(PathBuf::from("/root"), true, tx);
        runner.run(Effect::CheckHead);
        runner.run(Effect::CheckHead);
        runner.run(Effect::CheckHead);

        let action = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no head check in time")
            .expect("channel closed");
        assert!(matches!(action, Action::HeadChecked { .. }), "{action:?}");
        let extra = tokio::time::timeout(Duration::from_millis(800), rx.recv()).await;
        assert!(extra.is_err(), "the same burst was read again: {extra:?}");
    }

    #[tokio::test]
    async fn a_head_that_moved_again_is_reported_after_a_pause() {
        let dir = changed_repo();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(dir.path().to_path_buf(), true, tx);

        runner.run(Effect::CheckHead);
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no head check in time")
            .expect("channel closed");
        let Action::HeadChecked { oid: first_oid } = first else {
            panic!("expected a head check, got {first:?}");
        };
        assert!(first_oid.is_some(), "the commit was not seen");
        let reported = Instant::now();

        // The branch goes where `HEAD` no longer names it, which costs no
        // git of its own: the next read answers differently.
        std::fs::write(
            dir.path().join(".git/HEAD"),
            "ref: refs/heads/no-such-branch\n",
        )
        .expect("write");
        runner.run(Effect::CheckHead);
        let second = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no second head check in time")
            .expect("channel closed");
        let Action::HeadChecked { oid: second_oid } = second else {
            panic!("expected a head check, got {second:?}");
        };
        assert_ne!(first_oid, second_oid);
        let gap = reported.elapsed();
        assert!(
            gap >= HEAD_THROTTLE - Duration::from_millis(50),
            "the move was reported again after {gap:?}"
        );
    }

    #[tokio::test]
    async fn a_stats_answer_carries_the_generation_it_answers_for() {
        use crate::core::fsmodel::Status;

        let dir = changed_repo();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let runner = EffectRunner::new(dir.path().to_path_buf(), true, tx);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        runner.run(Effect::ComputeStats {
            kind: Kind::Head,
            at,
            paths: vec![(PathBuf::from("a.txt"), 4), (PathBuf::from("gone.txt"), 9)],
        });

        let action = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("no stats answer in time")
            .expect("channel closed");
        let Action::StatsComputed { results } = action else {
            panic!("expected a stats answer, got {action:?}");
        };
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].rel, PathBuf::from("a.txt"));
        assert_eq!(results[0].generation, 4);
        let changed = results[0].entry.as_ref().expect("a.txt is changed");
        assert_eq!(changed.status, Status::Modified);
        assert_eq!(changed.changed_at, at);
        // A path nothing is wrong with is answered too, so that it leaves
        // the changed set rather than staying as it was.
        assert_eq!(results[1].rel, PathBuf::from("gone.txt"));
        assert_eq!(results[1].generation, 9);
        assert!(results[1].entry.is_none());
    }

    async fn preview_loaded(
        rx: &mut tokio::sync::mpsc::Receiver<Action>,
    ) -> crate::core::preview::Doc {
        let action = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("no preview in time")
            .expect("channel closed");
        let Action::PreviewLoaded { result, .. } = action else {
            panic!("expected a preview result");
        };
        result.expect("the preview must load")
    }
}
