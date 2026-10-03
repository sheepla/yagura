//! The state behind a finder overlay: the query box, the list it filters and
//! the matcher that does the filtering (§13).

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config as MatcherConfig, Nucleo};
use ratatui::style::Style;
use ratatui_textarea::TextArea;

use crate::app::state::Overlay;
use crate::core::baseline::ChangedEntry;
use crate::features::finder::grep::GrepResults;

/// Characters a paste may add to what the query box already holds (§13.1).
pub const PASTE_LIMIT: usize = 1024;

/// What the query box says while it is empty, for each overlay (§13.1).
const FILES_PLACEHOLDER: &str = "type to filter files";
const GREP_PLACEHOLDER: &str = "type to search file contents";

/// How long a refresh waits for the matcher's workers before the list is
/// built from whatever they have finished so far.
const MATCH_TICK_MS: u64 = 4;

/// A query box, the list below it, and the matcher tying the two together.
#[derive(Default)]
pub struct FinderState {
    /// The query box (§13.1).
    pub query: TextArea<'static>,
    /// Every path the walk has answered with, in the order it arrived.
    pub found: Vec<PathBuf>,
    /// The fuzzy matcher over `found`, where an item is an index into it.
    /// Built when the overlay first opens, so a session that never opens one
    /// pays for neither the threads nor the memory.
    pub matcher: Option<Nucleo<u32>>,
    /// Indices into `found`, in the order the list shows them.
    pub shown: Vec<u32>,
    /// Which of them is selected; an index into `shown`.
    pub selected: usize,
    /// First row of `shown` that is on screen.
    pub scroll: usize,
    /// Generation of the walk this overlay is waiting for; a chunk carrying
    /// another one belongs to a walk that has been superseded (invariant I3).
    pub scan_gen: u64,
    /// Whether a walk is still going, which the list header says.
    pub scanning: bool,
    /// What the grep overlay's list holds; the other overlay never reads it
    /// (§13.3).
    pub grep: GrepResults,
    /// True while `shown` holds the walk order rather than the matcher's
    /// ranking, which is what an empty query lists (§7).
    walk_order: bool,
    /// The query changed and the matcher has not caught up with it yet.
    dirty: bool,
    /// The matcher may still have work to do, so the clock keeps it going.
    running: bool,
}

impl FinderState {
    /// Starts over for a new overlay and returns the walk generation it wants.
    ///
    /// Everything the last one held is dropped: the walk answers the
    /// generation that asked for it, and a query does not survive the overlay
    /// it was typed into.
    pub fn open(&mut self) -> u64 {
        self.query = query_box(FILES_PLACEHOLDER);
        self.found.clear();
        self.shown.clear();
        self.selected = 0;
        self.scroll = 0;
        self.walk_order = false;
        self.dirty = false;
        self.running = false;
        self.scanning = true;
        self.scan_gen = self.scan_gen.wrapping_add(1);
        if let Some(matcher) = self.matcher.as_mut() {
            matcher.restart(true);
        } else {
            self.matcher = Some(new_matcher());
        }
        self.scan_gen
    }

    /// Starts over for the grep overlay and returns the run it wants (§13.3).
    ///
    /// The list of the overlay this one replaces is dropped with everything
    /// else, and a walk still out there answers a generation nobody waits
    /// for: its chunks are dropped rather than listed under another overlay.
    pub fn open_grep(&mut self) -> u64 {
        self.query = query_box(GREP_PLACEHOLDER);
        self.found.clear();
        self.shown.clear();
        self.selected = 0;
        self.scroll = 0;
        self.walk_order = false;
        self.dirty = false;
        self.running = false;
        self.scanning = false;
        self.scan_gen = self.scan_gen.wrapping_add(1);
        self.grep.open()
    }

    /// The query as it stands.
    pub fn query_text(&self) -> String {
        self.query.lines().join("")
    }

    pub fn is_empty_query(&self) -> bool {
        self.query_text().is_empty()
    }

    /// The path the list has selected, for the overlay that is up: the file
    /// of a hit while the grep list is showing.
    pub fn selected_path(&self, overlay: Overlay) -> Option<PathBuf> {
        match overlay {
            Overlay::Grep => self.selected_match().map(|hit| hit.rel.clone()),
            _ => {
                let index = *self.shown.get(self.selected)?;
                self.found.get(index as usize).cloned()
            }
        }
    }

    /// The hit the selection is on, while the grep list is showing.
    pub fn selected_match(&self) -> Option<&crate::features::finder::grep::GrepMatch> {
        let index = *self.shown.get(self.selected)?;
        self.grep.matches.get(index as usize)
    }

    /// Whether the matcher may still be working on the current query.
    pub fn matcher_running(&self) -> bool {
        self.running
    }

    pub fn insert_char(&mut self, c: char) {
        self.query.insert_char(c);
    }

    /// Text from a paste: a query is one line, so the breaks in a pasted
    /// passage are dropped, and a box of a query is capped so that pasting a
    /// whole file costs nothing (§13.1).
    pub fn insert_text(&mut self, text: &str) {
        let room = PASTE_LIMIT.saturating_sub(self.query_text().chars().count());
        let kept: String = text
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .take(room)
            .collect();
        if !kept.is_empty() {
            self.query.insert_str(kept);
        }
    }

    pub fn delete_char(&mut self) {
        self.query.delete_char();
    }

    pub fn delete_next_char(&mut self) {
        self.query.delete_next_char();
    }

    pub fn delete_word(&mut self) {
        self.query.delete_word();
    }

    pub fn delete_to_start(&mut self) {
        self.query.delete_line_by_head();
    }

    pub fn delete_to_end(&mut self) {
        self.query.delete_line_by_end();
    }

    /// Takes one walk chunk into the list and into the matcher (§13.2).
    pub fn absorb(&mut self, rels: &[PathBuf], changed: &BTreeMap<PathBuf, ChangedEntry>) {
        // The matcher is built the first time a chunk arrives rather than at
        // construction, so a finder that is never opened costs nothing.
        let injector = self.matcher.get_or_insert_with(new_matcher).injector();
        for rel in rels {
            let index = self.found.len() as u32;
            // The column holds the path as the matcher reads it, which is the
            // whole of what a query is matched against.
            injector.push(index, |_, columns| {
                columns[0] = rel.to_string_lossy().as_ref().into()
            });
            self.found.push(rel.clone());
            // A list the matcher is showing is left alone: it will be ranked
            // again from the whole set once the query asks for it.
            if self.walk_order {
                self.insert_walk_order(index, changed);
            }
        }
        self.dirty = true;
    }

    /// Lists what the walk has found: the changed paths first, newest first,
    /// and the rest in the order the walk answered with (§7).
    ///
    /// A list that already holds the walk order grew while chunks arrived, so
    /// it is only ever built from the whole set when the query has just
    /// become empty again.
    fn list_walk_order(&mut self, changed: &BTreeMap<PathBuf, ChangedEntry>) {
        if self.walk_order {
            return;
        }
        let mut head: Vec<u32> = Vec::new();
        let mut tail: Vec<u32> = Vec::new();
        for (index, rel) in self.found.iter().enumerate() {
            if changed.contains_key(rel) {
                head.push(index as u32);
            } else {
                tail.push(index as u32);
            }
        }
        head.sort_by_key(|&index| Reverse(changed_at(changed, &self.found[index as usize])));
        head.extend(tail);
        self.shown = head;
        self.walk_order = true;
    }

    /// Puts one path where it belongs in a list that is already in walk
    /// order: into the changed run at the top, or at the end with the rest.
    fn insert_walk_order(&mut self, index: u32, changed: &BTreeMap<PathBuf, ChangedEntry>) {
        let Some(at) = changed
            .get(&self.found[index as usize])
            .map(|entry| entry.changed_at)
        else {
            self.shown.push(index);
            return;
        };
        let mut position = 0;
        while let Some(&other) = self.shown.get(position)
            && let Some(other_at) = changed
                .get(&self.found[other as usize])
                .map(|entry| entry.changed_at)
            && other_at >= at
        {
            position += 1;
        }
        self.shown.insert(position, index);
    }

    /// Lists what matches the query, ranked by the matcher (§7).
    fn list_matches(&mut self) {
        let query = self.query_text();
        let dirty = self.dirty;
        let matcher = self.matcher.get_or_insert_with(new_matcher);
        matcher
            .pattern
            .reparse(0, &query, CaseMatching::Smart, Normalization::Smart, false);
        let status = matcher.tick(MATCH_TICK_MS);
        if status.changed || dirty {
            self.shown = matcher
                .snapshot()
                .matched_items(..)
                .map(|item| *item.data)
                .collect();
        }
        if !status.running {
            self.dirty = false;
        }
        self.running = status.running;
        self.walk_order = false;
    }

    /// Brings the list in step with the query and the paths found so far.
    ///
    /// A grep list is ordered by `rg` and not by anything here, so only the
    /// part that keeps the selection inside it applies to that one (§13.3).
    pub fn refresh(&mut self, overlay: Overlay, changed: &BTreeMap<PathBuf, ChangedEntry>) {
        if overlay == Overlay::Grep {
            // `rg` gives the list its order and its contents; all that is
            // checked here is that every row still points at a hit (§13.3).
            let hits = self.grep.matches.len();
            self.shown.retain(|&index| (index as usize) < hits);
        } else if self.is_empty_query() {
            self.list_walk_order(changed);
        } else {
            self.list_matches();
        }
        if self.selected >= self.shown.len() {
            self.selected = self.shown.len().saturating_sub(1);
        }
    }

    /// Moves the selection, keeping it inside the list.
    pub fn select_by(&mut self, delta: isize) {
        if self.shown.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.shown.len() - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last as isize) as usize;
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.shown.len().saturating_sub(1);
    }

    /// Keeps the selected row on screen: the layout is a pure function of the
    /// terminal size, so the view never has to adjust itself (invariant I7).
    pub fn sync_scroll(&mut self, page: usize) {
        let page = page.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + page {
            self.scroll = self.selected + 1 - page;
        }
        self.scroll = self.scroll.min(self.shown.len().saturating_sub(page));
    }
}

/// The moment a path last changed against the baseline, or the beginning of
/// time for a path the baseline has nothing to say about.
fn changed_at(changed: &BTreeMap<PathBuf, ChangedEntry>, rel: &PathBuf) -> SystemTime {
    changed
        .get(rel)
        .map_or(SystemTime::UNIX_EPOCH, |entry| entry.changed_at)
}

/// The query box as an overlay opens it.
fn query_box(placeholder: &str) -> TextArea<'static> {
    let mut query = TextArea::default();
    query.set_cursor_line_style(Style::default());
    query.set_placeholder_text(placeholder);
    query
}

/// A matcher over one column: the path as the list shows it.
fn new_matcher() -> Nucleo<u32> {
    // Nothing has to happen when the matcher has news: the store asks for a
    // frame after the action that made the news anyway.
    Nucleo::new(MatcherConfig::DEFAULT, Arc::new(|| {}), None, 1)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::core::fsmodel::Status;

    fn entry(at: u64) -> ChangedEntry {
        ChangedEntry {
            status: Status::Modified,
            changed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(at),
            stat: None,
        }
    }

    fn changed(paths: &[(&str, u64)]) -> BTreeMap<PathBuf, ChangedEntry> {
        paths
            .iter()
            .map(|(rel, at)| (PathBuf::from(*rel), entry(*at)))
            .collect()
    }

    fn found(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn listed(finder: &FinderState) -> Vec<String> {
        finder
            .shown
            .iter()
            .map(|index| finder.found[*index as usize].to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn an_empty_query_lists_the_changed_paths_first_and_newest_first() {
        let mut finder = FinderState::default();
        let changed = changed(&[("mid.txt", 10), ("new.txt", 30)]);
        finder.absorb(
            &found(&["old.txt", "new.txt", "other.rs", "mid.txt"]),
            &changed,
        );
        finder.refresh(Overlay::Files, &changed);
        assert_eq!(
            listed(&finder),
            ["new.txt", "mid.txt", "old.txt", "other.rs"]
        );
    }

    #[test]
    fn a_path_found_later_joins_the_changed_run_at_the_top() {
        let mut finder = FinderState::default();
        let changed = changed(&[("mid.txt", 10), ("new.txt", 30)]);
        finder.absorb(&found(&["mid.txt", "other.rs"]), &changed);
        finder.refresh(Overlay::Files, &changed);
        finder.absorb(&found(&["new.txt"]), &changed);
        finder.refresh(Overlay::Files, &changed);
        assert_eq!(listed(&finder), ["new.txt", "mid.txt", "other.rs"]);
    }

    #[test]
    fn a_path_found_later_joins_the_rest_at_the_end() {
        let mut finder = FinderState::default();
        let changed = changed(&[("mid.txt", 10)]);
        finder.absorb(&found(&["mid.txt", "a.rs"]), &changed);
        finder.refresh(Overlay::Files, &changed);
        finder.absorb(&found(&["z.rs"]), &changed);
        finder.refresh(Overlay::Files, &changed);
        assert_eq!(listed(&finder), ["mid.txt", "a.rs", "z.rs"]);
    }

    #[test]
    fn going_back_to_an_empty_query_rebuilds_the_walk_order() {
        let mut finder = FinderState::default();
        let changed = changed(&[("new.txt", 30)]);
        finder.absorb(&found(&["a.rs", "new.txt"]), &changed);
        finder.refresh(Overlay::Files, &changed);
        // A query takes the list over: the matcher ranks it.
        finder.insert_char('n');
        finder.refresh(Overlay::Files, &changed);
        assert_eq!(listed(&finder), ["new.txt"]);
        // Back at no query the walk order comes back, changed first.
        finder.delete_char();
        finder.refresh(Overlay::Files, &changed);
        assert_eq!(listed(&finder), ["new.txt", "a.rs"]);
    }

    #[test]
    fn a_query_that_matches_nothing_lists_nothing() {
        let mut finder = FinderState::default();
        finder.absorb(&found(&["a.rs", "b.rs"]), &BTreeMap::new());
        finder.refresh(Overlay::Files, &BTreeMap::new());
        finder.insert_char('z');
        finder.refresh(Overlay::Files, &BTreeMap::new());
        assert!(finder.shown.is_empty());
        assert_eq!(finder.selected_path(Overlay::Files), None);
    }

    #[test]
    fn a_paste_loses_its_newlines_and_stops_at_the_limit() {
        let mut finder = FinderState::default();
        finder.insert_text("two\r\nlines\n");
        assert_eq!(finder.query_text(), "twolines");

        let long = "x".repeat(PASTE_LIMIT + 100);
        finder.insert_text(&long);
        assert_eq!(finder.query_text().chars().count(), PASTE_LIMIT);
    }

    #[test]
    fn the_selection_stays_inside_the_list() {
        let mut finder = FinderState::default();
        finder.absorb(&found(&["a.rs", "b.rs"]), &BTreeMap::new());
        finder.refresh(Overlay::Files, &BTreeMap::new());
        finder.select_by(1);
        finder.select_by(1);
        assert_eq!(finder.selected, 1, "the list ends at the last row");
        finder.select_by(-10);
        assert_eq!(finder.selected, 0, "the list starts at the first row");
        finder.select_last();
        assert_eq!(finder.selected, 1);
    }

    #[test]
    fn scrolling_keeps_the_selected_row_visible() {
        let mut finder = FinderState::default();
        let rels: Vec<String> = (0..20).map(|i| format!("f{i}.rs")).collect();
        let rels: Vec<PathBuf> = rels.iter().map(PathBuf::from).collect();
        finder.absorb(&rels, &BTreeMap::new());
        finder.refresh(Overlay::Files, &BTreeMap::new());

        finder.sync_scroll(5);
        assert_eq!(finder.scroll, 0);
        finder.selected = 12;
        finder.sync_scroll(5);
        assert_eq!(finder.scroll, 8, "the row below the window pulls it down");
        finder.selected = 0;
        finder.sync_scroll(5);
        assert_eq!(finder.scroll, 0, "the row above the window pulls it up");
        finder.select_last();
        finder.sync_scroll(5);
        assert_eq!(finder.scroll, 15, "the window stops at the last page");
    }

    #[test]
    fn a_new_overlay_starts_empty_and_changes_the_walk_generation() {
        let mut finder = FinderState::default();
        let first = finder.open();
        finder.absorb(&found(&["a.rs"]), &BTreeMap::new());
        finder.refresh(Overlay::Files, &BTreeMap::new());
        assert_eq!(listed(&finder), ["a.rs"]);

        let second = finder.open();
        assert_ne!(first, second);
        assert!(finder.shown.is_empty());
        assert!(finder.found.is_empty());
        assert!(finder.scanning);
        assert!(finder.query_text().is_empty());
    }

    #[test]
    fn a_matcher_only_appears_once_the_overlay_is_opened() {
        let mut finder = FinderState::default();
        assert!(finder.matcher.is_none());
        finder.open();
        assert!(finder.matcher.is_some());
    }

    #[test]
    fn a_query_box_says_what_it_wants() {
        assert_eq!(
            query_box(FILES_PLACEHOLDER).placeholder_text(),
            "type to filter files"
        );
        assert_eq!(
            query_box(GREP_PLACEHOLDER).placeholder_text(),
            "type to search file contents"
        );
    }

    fn hit(rel: &str, line: u32) -> crate::features::finder::grep::GrepMatch {
        crate::features::finder::grep::GrepMatch {
            rel: PathBuf::from(rel),
            line,
            text: String::new(),
            submatches: Vec::new(),
        }
    }

    #[test]
    fn a_grep_overlay_starts_another_list_of_its_own() {
        let mut finder = FinderState::default();
        finder.absorb(&found(&["a.rs"]), &BTreeMap::new());
        finder.refresh(Overlay::Files, &BTreeMap::new());

        let first = finder.open_grep();
        assert!(finder.shown.is_empty());
        assert!(finder.found.is_empty(), "the file list is put away");
        assert!(finder.query_text().is_empty(), "the box starts over");
        assert!(!finder.scanning, "no walk runs under a search");

        let second = finder.open_grep();
        assert_ne!(first, second, "another run, another generation");
    }

    #[test]
    fn a_grep_list_keeps_the_order_the_run_gave_it() {
        let mut finder = FinderState::default();
        let generation = finder.open_grep();
        finder.grep.matches = vec![hit("b.rs", 2), hit("a.rs", 1)];
        finder.shown = vec![0, 1];
        finder.insert_char('a');
        finder.refresh(Overlay::Grep, &BTreeMap::new());
        assert_eq!(finder.shown, [0, 1], "the matcher re-ranked the hits");
        assert!(finder.grep.answers(generation));

        // A list that loses every row must not keep a selection outside it.
        finder.grep.matches.clear();
        finder.selected = 1;
        finder.refresh(Overlay::Grep, &BTreeMap::new());
        assert!(finder.shown.is_empty());
        assert_eq!(finder.selected_path(Overlay::Grep), None);
        assert_eq!(finder.selected, 0);
    }
}
