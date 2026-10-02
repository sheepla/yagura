//! Running effects outside the UI loop.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use tokio::sync::mpsc::Sender;

use crate::app::action::Action;
use crate::app::effect::Effect;
use crate::core::baseline::{self, BaselineSource, Blob, Kind, Phase};
use crate::core::fswatch::watcher::FsWatcher;
use crate::core::preview::Baseline;
use crate::core::{dirload, preview};

/// How often the clock tick fires.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// The source of baseline content of the newest build, with its build number.
type BaselineSlot = Arc<Mutex<Option<(u64, Arc<dyn BaselineSource>)>>>;

/// Starts effects and returns their results as actions.
pub struct EffectRunner {
    root: PathBuf,
    ignore_enabled: bool,
    action_tx: Sender<Action>,
    baseline: BaselineSlot,
    /// The watch on the root, started by the first [`Effect::WatchDirs`] and
    /// kept alive for as long as the application runs.
    watcher: Mutex<Option<FsWatcher>>,
}

impl EffectRunner {
    pub fn new(root: PathBuf, ignore_enabled: bool, action_tx: Sender<Action>) -> Self {
        Self {
            root,
            ignore_enabled,
            action_tx,
            baseline: Arc::new(Mutex::new(None)),
            watcher: Mutex::new(None),
        }
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
                let root = self.root.clone();
                let tx = self.action_tx.clone();
                let slot = Arc::clone(&self.baseline);
                tokio::task::spawn_blocking(move || {
                    let built = baseline::build(&root, kind, SystemTime::now());
                    // The source has to answer before the reducer can ask for
                    // a diff, so it is stored under this build number before
                    // the result goes out. A build that already moved on keeps
                    // the source it stored.
                    if built.phase == Phase::Ready
                        && built.kind == Kind::Head
                        && let Some(source) = git_source(&root, &built)
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
        }
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
