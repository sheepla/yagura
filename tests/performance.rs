//! The performance acceptance criterion of M1: a directory with a hundred
//! thousand entries has to reach its first frame quickly, and a frame must only
//! ever build the rows that fit on the screen.
//!
//! The hundred thousand entries are made in memory rather than on disk, because
//! writing that many files makes the test slower than the thing it measures.

mod support;

use std::time::{Duration, Instant};

use yagura::app::{Action, Command, reduce::reduce};
use yagura::features::tree::state::MAX_VISIBLE_ROWS;

/// The size the acceptance criterion names.
const ENTRIES: usize = 100_000;

/// What the first frame is allowed to take.
const FIRST_FRAME_BUDGET: Duration = Duration::from_millis(100);

/// How many rows the tree pane shows on an 80x24 screen.
const ROWS_THAT_FIT: usize = 21;

#[test]
fn the_first_frame_of_a_hundred_thousand_entries_stays_inside_the_budget() {
    let mut state = support::state_with_entries(ENTRIES);
    state.tree.set_viewport(ROWS_THAT_FIT as u16);
    state.tree.refresh();
    assert_eq!(state.tree.row_count(), ENTRIES + 1, "every entry has a row");

    let start = Instant::now();
    let text = support::render_text(&state);
    let elapsed = start.elapsed();

    assert!(text.contains("dir0000"), "{text}");
    assert!(
        elapsed < FIRST_FRAME_BUDGET,
        "the first frame of {ENTRIES} entries took {elapsed:?}"
    );
}

#[test]
fn only_the_rows_that_fit_are_drawn() {
    let mut state = support::state_with_entries(ENTRIES);
    state.tree.set_viewport(ROWS_THAT_FIT as u16);
    state.tree.refresh();
    let text = support::render_text(&state);
    let drawn = text
        .lines()
        .filter(|line| line.contains("file") || line.contains("dir"))
        .count();
    assert_eq!(
        drawn, ROWS_THAT_FIT,
        "a frame of a huge tree may only draw the rows that fit"
    );
}

#[test]
fn scrolling_through_a_hundred_thousand_rows_is_responsive() {
    let mut state = support::state_with_entries(ENTRIES);
    state.tree.set_viewport(ROWS_THAT_FIT as u16);
    state.tree.refresh();

    let start = Instant::now();
    for _ in 0..50 {
        reduce(&mut state, Action::Command(Command::PageDown));
    }
    reduce(&mut state, Action::Command(Command::Last));
    let elapsed = start.elapsed();

    assert_eq!(state.tree.selected_index(), Some(ENTRIES));
    assert_eq!(state.tree.scroll, ENTRIES + 1 - ROWS_THAT_FIT);
    // Fifty page downs and a jump to the end, with a frame after each of them.
    assert!(
        elapsed < Duration::from_secs(5),
        "scrolling took {elapsed:?}, which would not feel instant"
    );
}

#[test]
fn a_following_selection_keeps_its_line() {
    let mut state = support::state_with_entries(ENTRIES);
    state.tree.set_viewport(ROWS_THAT_FIT as u16);
    state.tree.refresh();
    reduce(&mut state, Action::Command(Command::PageDown));
    let index = state.tree.selected_index().expect("a selected row");
    assert!(state.tree.scroll <= index && index < state.tree.scroll + ROWS_THAT_FIT);
}

#[test]
fn the_row_cache_is_capped() {
    // A pathological tree must not be able to exhaust memory: the cache stops
    // growing at the cap and the rows past it are simply not drawn.
    let state = support::state_with_entries(MAX_VISIBLE_ROWS + 50_000);
    assert_eq!(state.tree.row_count(), MAX_VISIBLE_ROWS);
    assert_eq!(state.tree.model.len(), MAX_VISIBLE_ROWS + 50_001);
}

#[test]
fn a_real_directory_of_five_thousand_files_is_read_and_sorted() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).expect("mkdir");
    for index in 0..5_000 {
        std::fs::write(root.join(format!("file{index:05}.txt")), b"x").expect("write");
    }
    for index in 0..50 {
        std::fs::create_dir(root.join(format!("dir{index:02}"))).expect("mkdir");
    }

    let state = support::loaded_state(&root, false);
    let root_node = state.tree.model.root_id();
    let children = state.tree.model.children(root_node);
    assert_eq!(children.len(), 5_050);
    // Directories first, then natural order by name.
    assert!(
        children[..50]
            .iter()
            .all(|id| { state.tree.model.get(*id).expect("node").is_expandable() })
    );
    assert!(state.tree.model.get(children[0]).expect("node").name == "dir00");

    let start = Instant::now();
    let text = support::render_text(&state);
    assert!(start.elapsed() < FIRST_FRAME_BUDGET);
    assert!(text.contains("dir00"), "{text}");
}
