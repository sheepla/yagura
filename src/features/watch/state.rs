//! The log of changes, kept whether or not the overlay is up (§12).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::core::fswatch::{ChangeKind, FsChange};

/// How many entries the log keeps; past this the oldest is dropped (§9).
pub const CAPACITY: usize = 10_000;

/// Two changes of one path and kind within this window are one entry (§12).
pub const MERGE_WINDOW: Duration = Duration::from_secs(1);

/// One line of the watch list: when, what, where, and how often (§9).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WatchEntry {
    pub at: SystemTime,
    pub kind: ChangeKind,
    pub rel: PathBuf,
    /// How many changes of this path and kind the entry stands for (§9).
    pub count: u32,
}

/// The log plus where the reader is in it (§12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchState {
    entries: VecDeque<WatchEntry>,
    /// The entry the reader is on, counted from the oldest entry kept.
    selected: usize,
    /// Whether the list keeps up with the newest entry (§12).
    follow: bool,
    /// How many entries arrived while it was not doing so (§12).
    unread: usize,
    /// The oldest entry the popup shows.
    scroll: usize,
}

impl Default for WatchState {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            selected: 0,
            // An empty log keeps up with whatever comes first.
            follow: true,
            unread: 0,
            scroll: 0,
        }
    }
}

impl WatchState {
    /// Takes a batch of changes. The log keeps recording while the overlay is
    /// down, so opening it later shows what happened in between (§12).
    pub fn record(&mut self, changes: &[FsChange], at: SystemTime) {
        for change in changes {
            if self.merge(change, at) {
                continue;
            }
            self.entries.push_back(WatchEntry {
                at,
                kind: change.kind,
                rel: change.rel.clone(),
                count: 1,
            });
            if self.entries.len() > CAPACITY {
                self.entries.pop_front();
                self.selected = self.selected.saturating_sub(1);
                self.scroll = self.scroll.saturating_sub(1);
            }
            if self.follow {
                self.selected = self.entries.len() - 1;
            } else {
                self.unread += 1;
            }
        }
    }

    /// The overlay opens on the newest entry, following it (§12).
    pub fn open(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
        self.follow = true;
        self.unread = 0;
    }

    /// Joins the change to the last entry when the two are the same path and
    /// kind within a second of each other (§12).
    fn merge(&mut self, change: &FsChange, at: SystemTime) -> bool {
        let Some(last) = self.entries.back_mut() else {
            return false;
        };
        if last.rel != change.rel || last.kind != change.kind || !within(last.at, at) {
            return false;
        }
        last.at = at;
        last.count += 1;
        true
    }

    /// Moves to the entry at `index`, which is where the reader has put the
    /// selection; whether the list follows comes from where that lands.
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.entries.len().saturating_sub(1));
        self.settle();
    }

    /// Up or down the log, stopping at both ends.
    pub fn select_by(&mut self, by: isize) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as isize - 1;
        let to = (self.selected as isize).saturating_add(by).clamp(0, last);
        self.select(to as usize);
    }

    /// The oldest entry of the log.
    pub fn select_first(&mut self) {
        self.select(0);
    }

    /// The newest entry, which is where following starts again (§9).
    pub fn select_last(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
        self.settle();
    }

    /// The list follows while the newest entry is the one selected, and
    /// counts what arrives when it is not (§12).
    fn settle(&mut self) {
        if self.entries.is_empty() {
            self.follow = true;
            return;
        }
        self.follow = self.selected + 1 == self.entries.len();
        if self.follow {
            self.unread = 0;
        }
    }

    /// Keeps the selected entry on screen, the way the tree does (§11).
    pub fn scroll_into_view(&mut self, viewport: usize) {
        if viewport == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + viewport {
            self.scroll = self.selected + 1 - viewport;
        }
        let last = self.entries.len().saturating_sub(viewport);
        if self.scroll > last {
            self.scroll = last;
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry at `index`, which is what a row of the list draws.
    pub fn get(&self, index: usize) -> Option<&WatchEntry> {
        self.entries.get(index)
    }

    pub fn selected(&self) -> Option<&WatchEntry> {
        self.entries.get(self.selected)
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn unread(&self) -> usize {
        self.unread
    }

    pub fn following(&self) -> bool {
        self.follow
    }
}

/// True when `at` is within [`MERGE_WINDOW`] of `since`, and false when the
/// clock has gone backwards across the two.
fn within(since: SystemTime, at: SystemTime) -> bool {
    at.duration_since(since)
        .is_ok_and(|passed| passed <= MERGE_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::fswatch::ChangeKind;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn change(rel: &str, kind: ChangeKind) -> FsChange {
        FsChange {
            rel: PathBuf::from(rel),
            kind,
        }
    }

    #[test]
    fn one_change_becomes_one_entry() {
        let mut watch = WatchState::default();
        watch.record(&[change("a.txt", ChangeKind::Modify)], at(10));
        assert_eq!(watch.len(), 1);
        let entry = watch.selected().expect("the entry");
        assert_eq!(entry.rel, PathBuf::from("a.txt"));
        assert_eq!(entry.kind, ChangeKind::Modify);
        assert_eq!(entry.count, 1);
        assert_eq!(entry.at, at(10));
    }

    #[test]
    fn the_same_change_within_a_second_stays_one_line_with_a_count() {
        let mut watch = WatchState::default();
        watch.record(&[change("a.txt", ChangeKind::Modify)], at(10));
        watch.record(&[change("a.txt", ChangeKind::Modify)], at(11));
        watch.record(&[change("a.txt", ChangeKind::Modify)], at(11));
        assert_eq!(watch.len(), 1, "three writes of one file");
        let entry = watch.selected().expect("the entry");
        assert_eq!(entry.count, 3);
        assert_eq!(entry.at, at(11), "the time of the last of them");
    }

    #[test]
    fn a_change_of_another_kind_or_of_another_time_is_a_line_of_its_own() {
        let mut watch = WatchState::default();
        watch.record(&[change("a.txt", ChangeKind::Modify)], at(10));
        watch.record(&[change("a.txt", ChangeKind::Remove)], at(10));
        assert_eq!(watch.len(), 2, "the kind differs");

        watch.record(&[change("b.txt", ChangeKind::Modify)], at(10));
        watch.record(&[change("b.txt", ChangeKind::Modify)], at(20));
        assert_eq!(watch.len(), 4, "a second has passed");

        watch.record(&[change("c.txt", ChangeKind::Create)], at(10));
        watch.record(&[change("c.txt", ChangeKind::Create)], at(5));
        assert_eq!(watch.len(), 6, "the clock went backwards");
    }

    #[test]
    fn the_log_keeps_what_the_capacity_allows_and_no_more() {
        let mut watch = WatchState::default();
        let changes: Vec<FsChange> = (0..CAPACITY + 10)
            .map(|index| change(&format!("f{index}.txt"), ChangeKind::Modify))
            .collect();
        watch.record(&changes, at(10));
        assert_eq!(watch.len(), CAPACITY, "the oldest were dropped");
        assert_eq!(
            watch.get(0).expect("the oldest kept").rel,
            PathBuf::from("f10.txt")
        );
        assert_eq!(watch.selected_index(), CAPACITY - 1, "still the newest");
    }

    #[test]
    fn the_list_follows_the_newest_entry_until_the_reader_moves_up() {
        let mut watch = WatchState::default();
        watch.record(&[change("a.txt", ChangeKind::Modify)], at(10));
        watch.record(&[change("b.txt", ChangeKind::Modify)], at(11));
        watch.open();
        assert!(watch.following());

        watch.select_by(-1);
        assert!(!watch.following(), "moving up leaves the following");

        watch.record(&[change("c.txt", ChangeKind::Modify)], at(12));
        watch.record(&[change("d.txt", ChangeKind::Modify)], at(13));
        assert_eq!(watch.unread(), 2, "what arrived while it was not looking");
        assert_eq!(
            watch.selected().expect("the entry").rel,
            PathBuf::from("a.txt")
        );

        watch.select_last();
        assert!(watch.following(), "the newest entry is followed again");
        assert_eq!(watch.unread(), 0, "nothing is unread at the end");
    }

    #[test]
    fn an_open_overlay_shows_the_newest_entry() {
        let mut watch = WatchState::default();
        watch.record(
            &[
                change("a.txt", ChangeKind::Modify),
                change("b.txt", ChangeKind::Create),
            ],
            at(10),
        );
        watch.select_first();
        watch.open();
        assert_eq!(
            watch.selected().expect("the entry").rel,
            PathBuf::from("b.txt")
        );
        assert_eq!(watch.unread(), 0);
    }

    #[test]
    fn the_row_that_is_read_stays_the_row_that_is_selected() {
        let mut watch = WatchState::default();
        watch.record(&[change("a.txt", ChangeKind::Modify)], at(10));
        watch.open();
        watch.record(&[change("b.txt", ChangeKind::Create)], at(11));
        assert_eq!(
            watch.selected().expect("the entry").rel,
            PathBuf::from("b.txt")
        );
    }

    #[test]
    fn a_row_that_does_not_fit_is_moved_onto_the_screen() {
        let mut watch = WatchState::default();
        let changes: Vec<FsChange> = (0..100)
            .map(|index| change(&format!("f{index}.txt"), ChangeKind::Modify))
            .collect();
        watch.record(&changes, at(10));
        watch.scroll_into_view(10);
        assert_eq!(watch.scroll(), 90, "the selection is the last line");

        watch.select_first();
        watch.scroll_into_view(10);
        assert_eq!(watch.scroll(), 0, "the first entry is on screen");

        watch.select(50);
        watch.scroll_into_view(10);
        assert_eq!(watch.scroll(), 41, "the selection is the last line again");

        watch.select_by(1);
        watch.scroll_into_view(10);
        assert_eq!(watch.scroll(), 42, "the view moves with the selection");
    }
}
