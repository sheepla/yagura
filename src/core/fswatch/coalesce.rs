//! Folding a stream of events into one batch per quiet period.
//!
//! A single save produces a handful of events and a build produces thousands
//! a second. The reducer only wants to know *what changed*, so events are
//! kept in a map keyed by path - one entry per path, no matter how often it
//! changes - and handed over as one action once the stream breathes (§7.2).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{ChangeKind, FsChange};

/// This long without an event and the batch is ready, however small.
pub const QUIET: Duration = Duration::from_millis(100);

/// No batch may grow older than this, so a steady stream still produces
/// frames instead of waiting for a quiet that never comes.
pub const MAX_AGE: Duration = Duration::from_millis(500);

/// Upper bound on the paths held at once. Past it nothing is remembered any
/// more and the tree is read again instead.
pub const MAX_PENDING: usize = 50_000;

/// What a path did between the first and the last event for it.
///
/// Only the ends matter: the reducer stats the path anyway, so the middle of
/// a burst cannot change the outcome (§7.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pending {
    first: ChangeKind,
    last: ChangeKind,
}

/// The end state of a path that was touched in several ways.
///
/// `None` means the path ended up as it began and there is nothing to say.
fn fold(first: ChangeKind, last: ChangeKind) -> Option<ChangeKind> {
    use ChangeKind::*;
    match (first, last) {
        // Created and gone again within one batch: nothing happened.
        (Create, Remove) => None,
        (Create, _) => Some(Create),
        // A rename or an atomic write is a removal that a create follows.
        (Remove, Create) => Some(Modify),
        (Modify, Create) => Some(Create),
        (Modify, Remove) => Some(Remove),
        (_, last) => Some(last),
    }
}

/// What a batch that is ready turns into.
#[derive(Debug, PartialEq, Eq)]
pub enum Taken {
    /// The paths that changed, sorted so that a batch reads the same way
    /// every time.
    Batch(Vec<FsChange>),
    /// Too much happened at once to remember: the whole tree has to be read
    /// again (§7.2).
    Rescan,
}

/// Pending events, keyed by path, with the times the flush rules need.
#[derive(Debug, Default)]
pub struct Coalescer {
    pending: HashMap<PathBuf, Pending>,
    /// When the oldest event in the batch arrived.
    first_at: Option<Instant>,
    /// When the newest event in the batch arrived.
    last_at: Option<Instant>,
    /// Set once the map has grown past [`MAX_PENDING`]: what was gathered
    /// was thrown away and nothing can be trusted any more.
    rescan: bool,
}

impl Coalescer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Files one event. The time comes from the caller so that the flush
    /// rules can be tested without waiting for a clock.
    pub fn push(&mut self, rel: PathBuf, kind: ChangeKind, now: Instant) {
        if self.rescan {
            // Nothing gathered from here on could be complete, so gathering
            // stops until the rescan has been handed over.
            return;
        }
        if self.pending.is_empty() {
            self.first_at = Some(now);
        }
        self.last_at = Some(now);
        let pending = self.pending.entry(rel).or_insert(Pending {
            first: kind,
            last: kind,
        });
        pending.last = kind;
        if self.pending.len() > MAX_PENDING {
            self.forget();
        }
    }

    /// Forgets the events and reports that the tree must be read again.
    pub fn request_rescan(&mut self) {
        if !self.rescan {
            self.forget();
        }
    }

    fn forget(&mut self) {
        self.pending.clear();
        self.first_at = None;
        self.last_at = None;
        self.rescan = true;
    }

    pub fn rescan(&self) -> bool {
        self.rescan
    }

    /// When the batch may be handed over, or `None` with nothing pending.
    pub fn deadline(&self) -> Option<Instant> {
        let first = self.first_at?;
        let last = self.last_at?;
        Some((last + QUIET).min(first + MAX_AGE))
    }

    /// Whether the batch is ready to go at `now`.
    pub fn ready(&self, now: Instant) -> bool {
        !self.pending.is_empty() && self.deadline().is_some_and(|at| now >= at)
    }

    /// Takes the batch if it is ready, so that the caller has only one place
    /// to ask from.
    pub fn flush(&mut self, now: Instant) -> Option<Taken> {
        if self.rescan {
            self.rescan = false;
            return Some(Taken::Rescan);
        }
        if !self.ready(now) {
            return None;
        }
        let pending = std::mem::take(&mut self.pending);
        self.first_at = None;
        self.last_at = None;
        let mut changes: Vec<FsChange> = pending
            .into_iter()
            .filter_map(|(rel, pending)| {
                fold(pending.first, pending.last).map(|kind| FsChange { rel, kind })
            })
            .collect();
        changes.sort_by(|a, b| a.rel.cmp(&b.rel));
        // A burst that folded away completely has nothing to say, and the
        // caller would rather wait than draw an empty frame.
        (!changes.is_empty()).then_some(Taken::Batch(changes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    /// The time an event at `offset` after the base arrived.
    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    fn batch(taken: Option<Taken>) -> Vec<FsChange> {
        match taken {
            Some(Taken::Batch(changes)) => changes,
            other => panic!("expected a batch, got {other:?}"),
        }
    }

    #[test]
    fn nothing_is_ready_before_the_quiet_period() {
        let base = Instant::now();
        let mut coalescer = Coalescer::new();
        coalescer.push(rel("a.txt"), ChangeKind::Modify, at(base, 0));
        assert!(!coalescer.ready(at(base, 99)));
        assert!(coalescer.ready(at(base, 100)));
        assert_eq!(
            coalescer.flush(at(base, 50)),
            None,
            "the batch went out before it was quiet"
        );
        assert_eq!(
            batch(coalescer.flush(at(base, 100))).len(),
            1,
            "the batch did not go out once it was quiet"
        );
        assert!(coalescer.flush(at(base, 100)).is_none());
    }

    #[test]
    fn a_deadline_without_a_quiet_period_flushes_a_steady_stream() {
        let base = Instant::now();
        let mut coalescer = Coalescer::new();
        // An event every 90ms: the quiet period never arrives on its own.
        for offset in 0..6 {
            coalescer.push(rel("a.txt"), ChangeKind::Modify, at(base, offset * 90));
        }
        // The last event keeps moving the quiet period away, but the oldest
        // one may not wait longer than `MAX_AGE`.
        assert_eq!(coalescer.deadline(), Some(at(base, 500)));
        assert_eq!(
            batch(coalescer.flush(at(base, 500))).len(),
            1,
            "the batch waited past its maximum age"
        );
    }

    #[test]
    fn one_path_holds_one_change() {
        let base = Instant::now();
        let mut coalescer = Coalescer::new();
        for (offset, kind) in [
            (0, ChangeKind::Create),
            (10, ChangeKind::Modify),
            (20, ChangeKind::Modify),
            (30, ChangeKind::Modify),
        ] {
            coalescer.push(rel("a.txt"), kind, at(base, offset));
        }
        coalescer.push(rel("b.txt"), ChangeKind::Create, at(base, 40));
        let mut changes = batch(coalescer.flush(at(base, 140)));
        changes.sort_by(|a, b| a.rel.cmp(&b.rel));
        assert_eq!(
            changes,
            vec![
                FsChange {
                    rel: rel("a.txt"),
                    kind: ChangeKind::Create
                },
                FsChange {
                    rel: rel("b.txt"),
                    kind: ChangeKind::Create
                },
            ],
            "a path keeps one entry no matter how often it changes"
        );
    }

    #[test]
    fn the_ends_of_a_burst_decide_what_is_reported() {
        let cases = [
            (
                vec![ChangeKind::Modify, ChangeKind::Remove],
                Some(ChangeKind::Remove),
            ),
            (
                vec![ChangeKind::Remove, ChangeKind::Create],
                Some(ChangeKind::Modify),
            ),
            (
                vec![ChangeKind::Create, ChangeKind::Modify],
                Some(ChangeKind::Create),
            ),
            (
                vec![ChangeKind::Modify, ChangeKind::Modify],
                Some(ChangeKind::Modify),
            ),
            (
                vec![ChangeKind::Remove, ChangeKind::Remove],
                Some(ChangeKind::Remove),
            ),
            // Created and gone again within one batch: nothing happened.
            (vec![ChangeKind::Create, ChangeKind::Remove], None),
        ];
        for (kinds, expected) in cases {
            let base = Instant::now();
            let mut coalescer = Coalescer::new();
            for (offset, kind) in kinds.iter().enumerate() {
                coalescer.push(rel("a.txt"), *kind, at(base, offset as u64));
            }
            let expected = match expected {
                Some(kind) => vec![FsChange {
                    rel: rel("a.txt"),
                    kind,
                }],
                None => Vec::new(),
            };
            let got = coalescer.flush(at(base, 200)).map(|taken| match taken {
                Taken::Batch(changes) => changes,
                Taken::Rescan => panic!("a rescan was not asked for"),
            });
            assert_eq!(got.unwrap_or_default(), expected, "{kinds:?}");
        }
    }

    #[test]
    fn an_overfull_map_asks_for_a_rescan_instead() {
        let base = Instant::now();
        let mut coalescer = Coalescer::new();
        for index in 0..MAX_PENDING + 1 {
            coalescer.push(
                rel(&format!("file{index}.txt")),
                ChangeKind::Modify,
                at(base, 0),
            );
        }
        assert!(coalescer.rescan());
        assert_eq!(coalescer.deadline(), None, "what was gathered is gone");
        assert_eq!(
            coalescer.flush(at(base, 0)),
            Some(Taken::Rescan),
            "the rescan is due immediately, not after the quiet period"
        );
        assert!(!coalescer.rescan());
    }

    #[test]
    fn nothing_is_gathered_while_a_rescan_is_pending() {
        let base = Instant::now();
        let mut coalescer = Coalescer::new();
        coalescer.push(rel("a.txt"), ChangeKind::Modify, at(base, 0));
        coalescer.request_rescan();
        // A batch that is not complete would be worse than no batch at all.
        coalescer.push(rel("b.txt"), ChangeKind::Create, at(base, 1));
        assert_eq!(coalescer.deadline(), None);
        assert_eq!(coalescer.flush(at(base, 1000)), Some(Taken::Rescan));
        assert_eq!(coalescer.flush(at(base, 1000)), None);
    }

    #[test]
    fn a_batch_is_sorted_so_that_it_reads_the_same_way_every_time() {
        let base = Instant::now();
        let mut coalescer = Coalescer::new();
        for name in ["z.txt", "a.txt", "m.txt"] {
            coalescer.push(rel(name), ChangeKind::Modify, at(base, 0));
        }
        let paths: Vec<PathBuf> = batch(coalescer.flush(at(base, 100)))
            .into_iter()
            .map(|change| change.rel)
            .collect();
        assert_eq!(paths, vec![rel("a.txt"), rel("m.txt"), rel("z.txt")]);
    }
}
