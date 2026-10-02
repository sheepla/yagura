//! The watch itself: one non-recursive watch per directory, and the task
//! that turns its events into actions (design §7.1).
//!
//! Every platform gets the same treatment: directories are watched one by
//! one, non-recursively, and only once their listing has been asked for. A
//! tree that is never opened - `node_modules` above all - is never watched,
//! and no watch limit can be eaten by a directory nobody looks at.
//!
//! A backend that watches more than it was asked to (FSEvents on macOS is
//! recursive by nature) is filtered back down here: an event is only filed
//! for a directory that was actually asked for, so the tree sees the same
//! thing on every platform.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Instant, SystemTime};

use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc::Sender;
use tokio::sync::{Notify, mpsc};

use super::ChangeKind;
use super::coalesce::{Coalescer, Taken};
use crate::app::action::Action;
use crate::core::dirload;

/// What the watcher thread and the flush task share.
struct Shared {
    /// The root every event is made relative to.
    root: PathBuf,
    /// Directories watched right now, relative to the root. The empty path
    /// is the root itself. An event whose directory is not in here is none
    /// of this watch's business.
    watched: Mutex<HashSet<PathBuf>>,
    coalescer: Mutex<Coalescer>,
    /// Wakes the flush task. `notify_one` stores a permit when nobody is
    /// waiting, so an event that arrives while the task is busy is never
    /// lost, and a burst still costs one wake up.
    wake: Notify,
}

impl Shared {
    fn watched(&self) -> std::sync::MutexGuard<'_, HashSet<PathBuf>> {
        self.watched.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn coalescer(&self) -> std::sync::MutexGuard<'_, Coalescer> {
        self.coalescer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Files one event. Called on the watcher's own thread (invariant I1).
    fn record(&self, event: &Event) {
        let mut filed = false;
        for (path, kind) in classify(event) {
            let Ok(rel) = path.strip_prefix(&self.root) else {
                // An event from outside the root has nothing to do with the
                // tree; a directory that was renamed away reports itself by
                // the path it was watched under.
                continue;
            };
            let Some(directory) = rel.parent() else {
                // The root itself: whatever happened there also happened to
                // an entry inside it, which is reported on its own.
                continue;
            };
            if !self.watched().contains(directory) {
                continue;
            }
            let Some(name) = rel.file_name() else {
                continue;
            };
            if dirload::is_skipped(name) {
                // Editor leftovers never reach a listing, so they must not
                // reach the reducer either. `.git` is not skipped here: the
                // name of the path `.git/HEAD` is `HEAD`.
                continue;
            }
            self.coalescer()
                .push(rel.to_path_buf(), kind, Instant::now());
            filed = true;
        }
        if filed {
            self.wake.notify_one();
        }
    }

    /// A watch reported an error: events may have been dropped, so nothing
    /// that was gathered can be trusted any more (§7.1).
    fn lost(&self, err: &notify::Error) {
        tracing::debug!(%err, "filesystem watch error");
        self.coalescer().request_rescan();
        self.wake.notify_one();
    }
}

/// The paths an event names, with what each of them did.
///
/// Only the end state of a path matters, so an event that says nothing about
/// it - a read - yields nothing at all (§7.2).
fn classify(event: &Event) -> Vec<(PathBuf, ChangeKind)> {
    use ChangeKind::*;
    match event.kind {
        EventKind::Access(_) => Vec::new(),
        EventKind::Create(_) => every(event, Create),
        EventKind::Remove(_) => every(event, Remove),
        EventKind::Modify(ModifyKind::Name(_)) if event.paths.len() >= 2 => {
            // A rename arrives as both of its ends: the old path is gone and
            // the new one is there.
            vec![
                (event.paths[0].clone(), Remove),
                (event.paths[1].clone(), Create),
            ]
        }
        EventKind::Modify(ModifyKind::Name(mode)) => match mode {
            RenameMode::From => every(event, Remove),
            RenameMode::To => every(event, Create),
            // One path and no direction: nothing better than a change.
            _ => every(event, Modify),
        },
        // Data, metadata, and anything a future notify version may add.
        _ => every(event, Modify),
    }
}

fn every(event: &Event, kind: ChangeKind) -> Vec<(PathBuf, ChangeKind)> {
    event
        .paths
        .iter()
        .map(|path| (path.clone(), kind))
        .collect()
}

/// The watch on one root, and the task the events are folded in.
pub struct FsWatcher {
    watcher: RecommendedWatcher,
    shared: Arc<Shared>,
    flush: tokio::task::JoinHandle<()>,
}

impl FsWatcher {
    /// Starts watching `root`, with the events sent to `tx` as actions.
    ///
    /// Only the root itself is watched at first. Every other directory is
    /// watched when its listing is requested, which is one moment before it
    /// is read, so that a file created during the read cannot slip through
    /// (§7.1).
    pub fn start(root: PathBuf, tx: Sender<Action>) -> notify::Result<Self> {
        let shared = Arc::new(Shared {
            root,
            watched: Mutex::new(HashSet::new()),
            coalescer: Mutex::new(Coalescer::new()),
            wake: Notify::new(),
        });
        let for_events = Arc::clone(&shared);
        let watcher = notify::recommended_watcher(move |result| match result {
            Ok(event) => for_events.record(&event),
            Err(err) => for_events.lost(&err),
        })?;
        let flush = tokio::spawn(flush_loop(Arc::clone(&shared), tx));
        let mut this = Self {
            watcher,
            shared,
            flush,
        };
        this.watch(Path::new(""))?;
        Ok(this)
    }

    /// Watches one directory, if it is not watched already.
    ///
    /// The directory is known as watched before the backend is told about
    /// it, so an event that races the call is kept rather than dropped.
    pub fn watch(&mut self, rel: &Path) -> notify::Result<()> {
        if self.shared.watched().contains(rel) {
            return Ok(());
        }
        self.shared.watched().insert(rel.to_path_buf());
        let path = self.path(rel);
        if let Err(err) = self.watcher.watch(&path, RecursiveMode::NonRecursive) {
            self.shared.watched().remove(rel);
            return Err(err);
        }
        Ok(())
    }

    /// Stops watching a directory and everything below it.
    ///
    /// A directory that was removed takes its own watches with it, but the
    /// set has to forget them: a later watch under the same path would be
    /// skipped as a duplicate and the directory would go unwatched.
    pub fn unwatch(&mut self, rel: &Path) {
        let stale: Vec<PathBuf> = self
            .shared
            .watched()
            .iter()
            .filter(|watched| watched.starts_with(rel))
            .cloned()
            .collect();
        for watched in stale {
            self.shared.watched().remove(&watched);
            let path = self.path(&watched);
            match self.watcher.unwatch(&path) {
                Ok(()) => {}
                // A directory that is already gone has already lost its
                // watch; the error says nothing new.
                Err(err) => tracing::debug!(%err, ?watched, "unwatch failed"),
            }
        }
    }

    /// The directory on disk a relative path stands for.
    fn path(&self, rel: &Path) -> PathBuf {
        if rel.as_os_str().is_empty() {
            self.shared.root.clone()
        } else {
            self.shared.root.join(rel)
        }
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        // The task would otherwise wait for events nobody collects any more.
        self.flush.abort();
    }
}

/// Hands the gathered events to the store, one action at a time (§7.2).
async fn flush_loop(shared: Arc<Shared>, tx: mpsc::Sender<Action>) {
    loop {
        let (taken, deadline) = {
            let mut coalescer = shared.coalescer();
            let taken = coalescer.flush(Instant::now());
            let deadline = coalescer.deadline();
            (taken, deadline)
        };
        if let Some(taken) = taken {
            let action = match taken {
                Taken::Batch(changes) => Action::FsBatch {
                    changes,
                    at: SystemTime::now(),
                },
                Taken::Rescan => Action::FsRescanNeeded,
            };
            if tx.send(action).await.is_err() {
                // The store is gone: nobody is watching the tree any more.
                return;
            }
        }
        // Waiting for the next event and for the batch to grow old at the
        // same time: a new event pulls the quiet period back, the maximum
        // age stops a busy tree from waiting forever.
        tokio::select! {
            _ = shared.wake.notified() => {}
            _ = sleep_until(deadline) => {}
        }
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::app::effect::Effect;
    use crate::app::runner::EffectRunner;
    use crate::core::fswatch::FsChange;

    fn change(action: &Action) -> &[FsChange] {
        match action {
            Action::FsBatch { changes, .. } => changes,
            other => panic!("expected a batch, got {other:?}"),
        }
    }

    /// A watcher whose store end is still open.
    fn watching(dir: &Path) -> (EffectRunner, mpsc::Receiver<Action>) {
        let (tx, rx) = mpsc::channel(64);
        let runner = EffectRunner::new(dir.to_path_buf(), true, tx);
        runner.run(Effect::WatchDirs {
            add: vec![PathBuf::new()],
            remove: Vec::new(),
        });
        (runner, rx)
    }

    /// Drains whatever the watcher has already sent.
    async fn settle(rx: &mut mpsc::Receiver<Action>) {
        while matches!(
            tokio::time::timeout(Duration::from_millis(250), rx.recv()).await,
            Ok(Some(_))
        ) {}
    }

    /// Whether an event arrives at all within `window`.
    async fn reports_within(rx: &mut mpsc::Receiver<Action>, window: Duration) -> bool {
        tokio::time::timeout(window, rx.recv()).await.is_ok()
    }

    #[tokio::test]
    async fn a_change_in_a_watched_directory_reaches_the_store_as_one_batch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (_runner, mut rx) = watching(dir.path());

        std::fs::write(dir.path().join("a.txt"), "x").expect("write");

        let action = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("no batch in time")
            .expect("channel closed");
        let changes = change(&action);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].rel, PathBuf::from("a.txt"));
    }

    #[tokio::test]
    async fn a_watched_directory_does_not_report_its_subdirectories() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(dir.path().join("inner")).expect("mkdir");
        let (_runner, mut rx) = watching(dir.path());
        settle(&mut rx).await;

        // Watching is directory by directory: what happens inside a
        // directory that was never opened is none of this watch's business.
        std::fs::write(dir.path().join("inner/child.txt"), "x").expect("write");
        assert!(
            !reports_within(&mut rx, Duration::from_millis(600)).await,
            "a subdirectory leaked into the batch"
        );
    }

    #[tokio::test]
    async fn an_unwatched_directory_stops_reporting() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (runner, mut rx) = watching(dir.path());

        std::fs::write(dir.path().join("a.txt"), "x").expect("write");
        let action = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("no batch in time")
            .expect("channel closed");
        assert!(!change(&action).is_empty());
        settle(&mut rx).await;

        runner.run(Effect::WatchDirs {
            add: Vec::new(),
            remove: vec![PathBuf::new()],
        });
        std::fs::write(dir.path().join("b.txt"), "x").expect("write");
        assert!(
            !reports_within(&mut rx, Duration::from_millis(600)).await,
            "an unwatched directory still reported"
        );
    }

    #[tokio::test]
    async fn watching_the_same_directory_twice_changes_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (runner, mut rx) = watching(dir.path());
        runner.run(Effect::WatchDirs {
            add: vec![PathBuf::new()],
            remove: Vec::new(),
        });

        std::fs::write(dir.path().join("a.txt"), "x").expect("write");
        let action = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("no batch in time")
            .expect("channel closed");
        let changes = change(&action);
        assert_eq!(changes.len(), 1, "{changes:?}");
    }

    #[tokio::test]
    async fn a_missing_root_does_not_take_the_application_down() {
        let (tx, mut rx) = mpsc::channel(64);
        let missing = PathBuf::from("/definitely/not/here");
        let runner = EffectRunner::new(missing, true, tx);
        runner.run(Effect::WatchDirs {
            add: vec![PathBuf::new()],
            remove: Vec::new(),
        });
        // No watcher started, so nothing ever arrives - and nothing else
        // happened either, which is the whole point.
        assert!(!reports_within(&mut rx, Duration::from_millis(300)).await);
    }
}
