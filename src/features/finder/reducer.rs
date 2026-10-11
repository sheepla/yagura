//! The finder reducer: what an overlay does with what happens to it.

use std::path::PathBuf;

use ratatui_textarea::CursorMove;

use crate::app::action::Action;
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::{AppState, Focus, Overlay};
use crate::features::finder::grep;
use crate::features::tree;
use crate::ui::layout;

/// Whether a command belongs to the finder rather than to the panes behind
/// it.
///
/// Scrolling and `Esc` are not here: the overlay borrows those from the
/// panes instead of taking them over, so that one key means one thing (§4).
pub fn owns(command: Command) -> bool {
    matches!(
        command,
        Command::OpenFiles
            | Command::OpenGrep
            | Command::FinderAccept
            | Command::FinderSelectUp
            | Command::FinderSelectDown
            | Command::FinderPageUp
            | Command::FinderPageDown
            | Command::FinderInsert(_)
            | Command::FinderCursorLeft
            | Command::FinderCursorRight
            | Command::FinderCursorWordLeft
            | Command::FinderCursorWordRight
            | Command::FinderCursorHome
            | Command::FinderCursorEnd
            | Command::FinderDeleteBack
            | Command::FinderDeleteForward
            | Command::FinderDeleteWord
            | Command::FinderDeleteToStart
            | Command::FinderDeleteToEnd
    )
}

/// Applies one action that concerns the finder.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    match action {
        Action::Command(command) => on_command(state, *command),
        Action::Paste(text) => {
            state.finder.insert_text(text);
            query_changed(state)
        }
        Action::FilesFound { generation, rels } => on_found(state, *generation, rels),
        Action::FilesScanDone { generation } => {
            if *generation == state.finder.scan_gen {
                state.finder.scanning = false;
            }
            Vec::new()
        }
        Action::GrepChunk {
            generation,
            matches,
        } => on_chunk(state, *generation, matches),
        Action::GrepDone {
            generation,
            truncated,
        } => on_done(state, *generation, *truncated),
        Action::GrepFailed {
            generation,
            message,
        } => on_failed(state, *generation, message),
        Action::GrepMissing {
            generation,
            message,
        } => on_missing(state, *generation, message),
        Action::Tick { .. } => {
            refresh(state);
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn on_command(state: &mut AppState, command: Command) -> Vec<Effect> {
    match command {
        // The commands that have work of their own to ask for.
        Command::OpenFiles => return open(state),
        Command::OpenGrep => return open_grep(state),
        Command::FinderAccept => return accept(state),
        _ => {}
    }
    match command {
        Command::FinderSelectUp => {
            state.finder.select_by(-1);
            sync(state);
        }
        Command::FinderSelectDown => {
            state.finder.select_by(1);
            sync(state);
        }
        Command::FinderPageUp => {
            let page = page(state);
            state.finder.select_by(-(page as isize));
            sync(state);
        }
        Command::FinderPageDown => {
            let page = page(state);
            state.finder.select_by(page as isize);
            sync(state);
        }
        Command::FinderInsert(c) => {
            state.finder.insert_char(c);
            return query_changed(state);
        }
        Command::FinderCursorLeft => state.finder.query.move_cursor(CursorMove::Back),
        Command::FinderCursorRight => state.finder.query.move_cursor(CursorMove::Forward),
        Command::FinderCursorWordLeft => state.finder.query.move_cursor(CursorMove::WordBack),
        Command::FinderCursorWordRight => state.finder.query.move_cursor(CursorMove::WordForward),
        Command::FinderCursorHome => state.finder.query.move_cursor(CursorMove::Head),
        Command::FinderCursorEnd => state.finder.query.move_cursor(CursorMove::End),
        Command::FinderDeleteBack => {
            state.finder.delete_char();
            return query_changed(state);
        }
        Command::FinderDeleteForward => {
            state.finder.delete_next_char();
            return query_changed(state);
        }
        Command::FinderDeleteWord => {
            state.finder.delete_word();
            return query_changed(state);
        }
        Command::FinderDeleteToStart => {
            state.finder.delete_to_start();
            return query_changed(state);
        }
        Command::FinderDeleteToEnd => {
            state.finder.delete_to_end();
            return query_changed(state);
        }
        _ => {}
    }
    Vec::new()
}

/// `Ctrl+P`: the overlay takes the screen and a walk starts under it (§13.2).
fn open(state: &mut AppState) -> Vec<Effect> {
    state.overlay = Overlay::Files;
    let generation = state.finder.open();
    // The overlay draws its own preview pane, so the height the preview
    // scrolls by is the one now on screen rather than the pane behind it.
    crate::app::reduce::sync_preview_viewport(state);
    refresh(state);
    vec![Effect::ScanFiles { generation }]
}

/// `Ctrl+G`: from the panes the grep list takes the screen, and inside the
/// overlay the same key is the switch between the two lists (§13.3).
fn open_grep(state: &mut AppState) -> Vec<Effect> {
    match state.overlay {
        Overlay::Grep => open(state),
        _ => show_grep(state),
    }
}

/// The grep overlay takes the screen; without `rg` it says why instead (§13.3).
fn show_grep(state: &mut AppState) -> Vec<Effect> {
    if let Some(reason) = state.finder.grep.missing.clone() {
        state.set_error(reason);
        return Vec::new();
    }
    state.overlay = Overlay::Grep;
    state.finder.open_grep();
    crate::app::reduce::sync_preview_viewport(state);
    search(state)
}

/// Starts the run the query asks for, or nothing when it asks for none (§7).
///
/// Every query supersedes the one before it: the generation moves on, so
/// whatever the old run still has to say is dropped rather than mixed into
/// the answers of this one (invariant I3).
fn search(state: &mut AppState) -> Vec<Effect> {
    let pattern = state.finder.query_text();
    let generation = state.finder.grep.start_run();
    if pattern.chars().count() < grep::MIN_PATTERN {
        // Below two characters there is no search, and the answers of a
        // query that is gone are no answers of this one (§7).
        state.finder.grep.matches.clear();
        state.finder.grep.truncated = false;
        state.finder.grep.error = None;
        state.finder.shown.clear();
        state.finder.select_first();
        state.finder.scroll = 0;
        refresh(state);
        return Vec::new();
    }
    // A run that has not been asked for yet cannot have failed yet.
    state.finder.grep.error = None;
    state.finder.grep.running = true;
    refresh(state);
    vec![Effect::StartGrep {
        generation,
        pattern,
    }]
}

/// `Enter`: the row becomes the selection and the preview (§4).
///
/// With nothing in the list there is nowhere to go, and `Enter` does nothing
/// (§7).
fn accept(state: &mut AppState) -> Vec<Effect> {
    if state.overlay == Overlay::Grep {
        return accept_grep(state);
    }
    let overlay = state.overlay;
    let Some(rel) = state.finder.selected_path(overlay) else {
        return Vec::new();
    };
    state.overlay = Overlay::None;
    crate::app::reduce::sync_preview_viewport(state);
    state.focus = Focus::Preview;
    let mut effects = Vec::new();
    tree::reducer::jump_to(state, rel, &mut effects);
    effects
}

/// `Enter` on a hit: the overlay comes down and the pane is aimed at the line
/// the row was on, which is where the reader lands (§4, §13.3).
///
/// The preview mode is left alone: what is on screen is the file, not the
/// way a diff would draw it.
fn accept_grep(state: &mut AppState) -> Vec<Effect> {
    let Some(hit) = state.finder.selected_match().cloned() else {
        return Vec::new();
    };
    state.overlay = Overlay::None;
    crate::app::reduce::sync_preview_viewport(state);
    state.focus = Focus::Preview;
    state.preview.aim_at(Some(&hit));
    let mut effects = Vec::new();
    tree::reducer::jump_to(state, hit.rel, &mut effects);
    effects
}

/// One walk chunk: paths the list shows before the walk has finished (§7).
fn on_found(state: &mut AppState, generation: u64, rels: &[PathBuf]) -> Vec<Effect> {
    // A walk the overlay has been reopened without is still running; its
    // answer belongs to a generation nobody is waiting for (invariant I3).
    if generation != state.finder.scan_gen || state.overlay != Overlay::Files {
        return Vec::new();
    }
    state.finder.absorb(rels, &state.baseline.changed);
    refresh(state);
    Vec::new()
}

/// One batch of hits: rows the list shows while the run is still going (§7).
fn on_chunk(state: &mut AppState, generation: u64, matches: &[grep::GrepMatch]) -> Vec<Effect> {
    // A run the overlay has been left, or a query typed over, is still out
    // there; its answer belongs to nobody (invariant I3).
    if !state.finder.grep.answers(generation) || state.overlay != Overlay::Grep {
        return Vec::new();
    }
    if !state.finder.grep.answered {
        // The first answer of this run replaces the list rather than joining
        // it: the rows still on screen are answers to the last query, and
        // the hits behind them have nobody to answer to any more.
        state.finder.grep.answered = true;
        state.finder.grep.matches.clear();
        state.finder.shown.clear();
        state.finder.select_first();
        state.finder.scroll = 0;
    }
    let first = state.finder.grep.matches.len();
    state.finder.grep.matches.extend_from_slice(matches);
    for index in first..state.finder.grep.matches.len() {
        state.finder.shown.push(index as u32);
    }
    refresh(state);
    Vec::new()
}

/// The run is over: the list stops saying that it is going (§13.3).
fn on_done(state: &mut AppState, generation: u64, truncated: bool) -> Vec<Effect> {
    if !state.finder.grep.answers(generation) || state.overlay != Overlay::Grep {
        return Vec::new();
    }
    if !state.finder.grep.answered {
        // Nothing came back at all, which is its own answer (§7).
        state.finder.grep.answered = true;
        state.finder.shown.clear();
        state.finder.select_first();
    }
    state.finder.grep.truncated |= truncated;
    state.finder.grep.running = false;
    refresh(state);
    Vec::new()
}

/// `rg` could not take the pattern: the list says so rather than hiding it (§7).
fn on_failed(state: &mut AppState, generation: u64, message: &str) -> Vec<Effect> {
    if !state.finder.grep.answers(generation) || state.overlay != Overlay::Grep {
        return Vec::new();
    }
    state.finder.grep.error = Some(message.to_owned());
    state.finder.grep.matches.clear();
    state.finder.shown.clear();
    state.finder.grep.running = false;
    refresh(state);
    Vec::new()
}

/// `rg` is not installed: the overlay comes down and the status bar says why
/// (§13.3).
fn on_missing(state: &mut AppState, generation: u64, message: &str) -> Vec<Effect> {
    if !state.finder.grep.answers(generation) {
        return Vec::new();
    }
    // It says something about the machine, so it is remembered even when the
    // overlay has already been closed by hand.
    state.finder.grep.missing = Some(message.to_owned());
    if state.overlay != Overlay::Grep {
        return Vec::new();
    }
    state.set_error(message);
    state.overlay = Overlay::None;
    crate::app::reduce::sync_preview_viewport(state);
    refresh(state);
    Vec::new()
}

/// The query text changed: the list starts again from the first row, so the
/// first row is always the best answer to what is typed now.
///
/// In the grep overlay it is also the moment the old run stops mattering
/// (§13.3).
fn query_changed(state: &mut AppState) -> Vec<Effect> {
    state.finder.select_first();
    state.finder.scroll = 0;
    if state.overlay == Overlay::Grep {
        return search(state);
    }
    refresh(state);
    Vec::new()
}

/// Brings the list, the selection and the window over it in step.
fn refresh(state: &mut AppState) {
    state.finder.refresh(state.overlay, &state.baseline.changed);
    sync(state);
}

fn sync(state: &mut AppState) {
    state.finder.sync_scroll(page(state));
}

/// Rows the list has, which is what a page of it is.
fn page(state: &AppState) -> usize {
    layout::list_rows(state.area()) as usize
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::app::state::Config;
    use crate::cli::{BaselineKind, IconStyle};
    use crate::core::fsmodel::{DirEntry, NodeKind};

    fn state() -> AppState {
        AppState::new(
            Config {
                root: PathBuf::from("/root"),
                icons: IconStyle::Text,
                baseline: BaselineKind::Auto,
                ignore_enabled: true,
                no_color: false,
                log: None,
                selftest_panic: false,
            },
            (80, 24),
        )
    }

    fn entry(name: &str, kind: NodeKind) -> DirEntry {
        DirEntry {
            name: name.into(),
            kind,
            link_target: None,
            unreadable: false,
        }
    }

    /// The paths the list shows, in the order it shows them.
    fn listed(state: &AppState) -> Vec<String> {
        state
            .finder
            .shown
            .iter()
            .map(|index| {
                state.finder.found[*index as usize]
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn open_overlay(state: &mut AppState) -> Vec<Effect> {
        crate::app::reduce::reduce(state, Action::Command(Command::OpenFiles))
    }

    /// One walk chunk, under the generation the overlay is waiting for.
    fn chunk(state: &mut AppState, rels: &[&str]) -> Vec<Effect> {
        let generation = state.finder.scan_gen;
        let rels: Vec<PathBuf> = rels.iter().map(PathBuf::from).collect();
        crate::app::reduce::reduce(state, Action::FilesFound { generation, rels })
    }

    /// The root listing in hand, with a `src` directory below it.
    fn listed_root(state: &mut AppState) {
        let root = state.tree.model.root_id();
        let generation = state.tree.load_gen[&root];
        crate::app::reduce::reduce(
            state,
            Action::DirLoaded {
                node: root,
                generation,
                result: Ok(vec![
                    entry("README.md", NodeKind::File),
                    entry("src", NodeKind::Dir),
                ]),
            },
        );
    }

    #[test]
    fn opening_the_files_overlay_takes_the_screen_and_starts_a_walk() {
        let mut state = state();
        let effects = open_overlay(&mut state);
        assert_eq!(state.overlay, Overlay::Files);
        assert!(state.finder.scanning, "the header says the walk is going");
        assert_eq!(
            effects,
            vec![Effect::ScanFiles {
                generation: state.finder.scan_gen
            }],
            "one walk, under the generation the overlay asks for"
        );
    }

    #[test]
    fn the_list_grows_as_the_walk_answers_and_stale_answers_are_dropped() {
        let mut state = state();
        open_overlay(&mut state);
        chunk(&mut state, &["src/main.rs"]);
        chunk(&mut state, &["README.md"]);
        assert_eq!(listed(&state), ["src/main.rs", "README.md"]);

        // Reopening supersedes the walk that is still out there: its chunks
        // and its end are both from a generation nobody waits for (I3).
        let stale = state.finder.scan_gen;
        open_overlay(&mut state);
        crate::app::reduce::reduce(
            &mut state,
            Action::FilesFound {
                generation: stale,
                rels: vec![PathBuf::from("stale.txt")],
            },
        );
        crate::app::reduce::reduce(&mut state, Action::FilesScanDone { generation: stale });
        assert!(state.finder.found.is_empty(), "no stale chunk was taken");
        assert!(state.finder.scanning, "no stale end stopped this walk");

        chunk(&mut state, &["src/main.rs"]);
        let generation = state.finder.scan_gen;
        crate::app::reduce::reduce(&mut state, Action::FilesScanDone { generation });
        assert!(!state.finder.scanning, "this walk has answered its end");
        assert_eq!(listed(&state), ["src/main.rs"]);
    }

    #[test]
    fn a_query_starts_the_list_over_from_the_top() {
        let mut state = state();
        open_overlay(&mut state);
        chunk(&mut state, &["a.rs", "b.rs", "c.rs"]);
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderSelectDown));
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderSelectDown));
        assert_eq!(state.finder.selected, 2);

        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('b')));
        assert_eq!(state.finder.query_text(), "b");
        assert_eq!(state.finder.selected, 0, "the best answer is the first row");
        assert_eq!(state.finder.scroll, 0);

        // A clock tick keeps the matcher going and changes nothing else.
        let now = state.now;
        let effects = crate::app::reduce::reduce(&mut state, Action::Tick { now });
        assert!(effects.is_empty());
        assert_eq!(state.finder.query_text(), "b");
    }

    #[test]
    fn a_paste_is_one_line_of_query_text() {
        let mut state = state();
        open_overlay(&mut state);
        crate::app::reduce::reduce(&mut state, Action::Paste("src/m\nain.rs\n".to_owned()));
        assert_eq!(state.finder.query_text(), "src/main.rs");
        assert_eq!(state.finder.selected, 0);
    }

    /// Every key the query box has reaches the box: the characters, the
    /// cursors and the three ways of taking text out (§13.1).
    #[test]
    fn every_editing_key_reaches_the_query_box() {
        let mut state = state();
        open_overlay(&mut state);
        let edit = |state: &mut AppState, command: Command| {
            crate::app::reduce::reduce(state, Action::Command(command));
        };

        for c in "the quick".chars() {
            edit(&mut state, Command::FinderInsert(c));
        }
        assert_eq!(state.finder.query_text(), "the quick");

        // Home takes the cursor to the head, forward delete the first letter.
        edit(&mut state, Command::FinderCursorHome);
        edit(&mut state, Command::FinderDeleteForward);
        assert_eq!(state.finder.query_text(), "he quick");

        // End takes it back to the tail, backspace the last letter.
        edit(&mut state, Command::FinderCursorEnd);
        edit(&mut state, Command::FinderDeleteBack);
        assert_eq!(state.finder.query_text(), "he quic");

        // Left and right step over single characters, which the two inserts
        // below land exactly where the cursor was left.
        edit(&mut state, Command::FinderCursorLeft);
        edit(&mut state, Command::FinderInsert('|'));
        assert_eq!(state.finder.query_text(), "he qui|c");
        edit(&mut state, Command::FinderCursorRight);
        edit(&mut state, Command::FinderInsert('/'));
        assert_eq!(state.finder.query_text(), "he qui|c/");

        // Word left from the tail walks to the head of the word before it.
        edit(&mut state, Command::FinderCursorHome);
        edit(&mut state, Command::FinderDeleteToEnd);
        for c in "one two".chars() {
            edit(&mut state, Command::FinderInsert(c));
        }
        edit(&mut state, Command::FinderCursorWordLeft);
        edit(&mut state, Command::FinderInsert('['));
        assert_eq!(state.finder.query_text(), "one [two");

        // Word right from the head walks over the word it stands on.
        edit(&mut state, Command::FinderCursorHome);
        edit(&mut state, Command::FinderDeleteToEnd);
        for c in "alpha beta".chars() {
            edit(&mut state, Command::FinderInsert(c));
        }
        edit(&mut state, Command::FinderCursorHome);
        edit(&mut state, Command::FinderCursorWordRight);
        edit(&mut state, Command::FinderInsert('{'));
        assert_eq!(state.finder.query_text(), "alpha {beta");

        // A word at the cursor goes at once...
        edit(&mut state, Command::FinderCursorHome);
        edit(&mut state, Command::FinderDeleteToEnd);
        for c in "alpha beta".chars() {
            edit(&mut state, Command::FinderInsert(c));
        }
        edit(&mut state, Command::FinderDeleteWord);
        assert_eq!(state.finder.query_text(), "alpha ");

        // ...the way back to the head goes at once...
        edit(&mut state, Command::FinderDeleteToStart);
        assert_eq!(state.finder.query_text(), "");

        // ...and so does the way to the tail.
        for c in "again".chars() {
            edit(&mut state, Command::FinderInsert(c));
        }
        edit(&mut state, Command::FinderCursorHome);
        edit(&mut state, Command::FinderDeleteToEnd);
        assert_eq!(state.finder.query_text(), "");
    }

    /// The page keys move the list by a page at a time, and stop at the ends
    /// the single-step keys stop at (§13.2).
    #[test]
    fn the_page_keys_move_the_list_by_a_page() {
        let mut state = state();
        open_overlay(&mut state);
        let paths: Vec<String> = (0..100).map(|index| format!("f{index:03}.txt")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        chunk(&mut state, &refs);
        assert_eq!(state.finder.shown.len(), 100);

        let page = page(&state);
        assert!(page > 0, "the popup has rows");
        state.finder.selected = page;
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderPageDown));
        assert_eq!(state.finder.selected, page * 2, "a page down");
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderPageUp));
        assert_eq!(state.finder.selected, page, "and a page up");

        state.finder.selected = 0;
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderPageUp));
        assert_eq!(state.finder.selected, 0, "the top of the list");
    }

    #[test]
    fn enter_takes_the_row_all_the_way_to_the_tree_and_the_preview() {
        let mut state = state();
        listed_root(&mut state);
        open_overlay(&mut state);
        let found = chunk(&mut state, &["src/main.rs"]);
        assert!(
            found.iter().any(|effect| matches!(
                effect,
                Effect::LoadPreview { rel, .. } if rel == Path::new("src/main.rs")
            )),
            "the preview follows the row the overlay has selected: {found:?}"
        );

        let effects =
            crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderAccept));
        assert_eq!(state.overlay, Overlay::None);
        assert_eq!(
            state.focus,
            Focus::Preview,
            "the reader lands in the preview"
        );
        assert_eq!(
            state.tree.jump,
            Some(PathBuf::from("src/main.rs")),
            "the jump waits for the listing that stands in its way"
        );
        assert_eq!(
            state.preview.path.as_deref(),
            Some(Path::new("src/main.rs")),
            "the file the row held is what the preview is showing"
        );
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::LoadDir { rel, .. } if rel == Path::new("src")
            )),
            "the way down is asked for: {effects:?}"
        );

        // The listing arrives and the jump lands on the row.
        let src = state.tree.model.lookup(Path::new("src")).expect("src");
        let generation = state.tree.load_gen[&src];
        crate::app::reduce::reduce(
            &mut state,
            Action::DirLoaded {
                node: src,
                generation,
                result: Ok(vec![entry("main.rs", NodeKind::File)]),
            },
        );
        let target = state
            .tree
            .model
            .lookup(Path::new("src/main.rs"))
            .expect("main.rs");
        assert_eq!(state.tree.jump, None, "the jump has landed");
        assert_eq!(state.tree.selected, Some(target));
        assert_eq!(
            state.preview.path.as_deref(),
            Some(Path::new("src/main.rs"))
        );
    }

    #[test]
    fn enter_with_nothing_in_the_list_does_nothing() {
        let mut state = state();
        open_overlay(&mut state);
        let effects =
            crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderAccept));
        assert_eq!(state.overlay, Overlay::Files, "the overlay stays up");
        assert!(state.tree.jump.is_none());
        assert!(effects.is_empty(), "§7: there is nowhere to go");
    }

    #[test]
    fn enter_turns_off_a_filter_that_would_hide_the_row_it_jumps_to() {
        let mut state = state();
        state.tree.changed_only = true;
        open_overlay(&mut state);
        chunk(&mut state, &["src/main.rs"]);
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderAccept));
        assert!(!state.tree.changed_only);
        assert!(
            state
                .visible_message()
                .is_some_and(|message| message.text.contains("filter was turned off")),
            "the reader is told why the filter went away"
        );
    }

    #[test]
    fn a_jump_to_a_file_no_listing_has_cancels_itself() {
        let mut state = state();
        listed_root(&mut state);
        open_overlay(&mut state);
        // The walk has found a path the tree has no row for, and the listing
        // that would hold it is already in hand: waiting forever would keep
        // the jump pending for a file that is gone.
        chunk(&mut state, &["gone.txt"]);
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderAccept));
        assert_eq!(state.tree.jump, None);
        assert_eq!(state.overlay, Overlay::None);
    }

    /// One hit, as a run reports it.
    fn hit(rel: &str, line: u32) -> grep::GrepMatch {
        grep::GrepMatch {
            rel: PathBuf::from(rel),
            line,
            text: "fn main() {".to_owned(),
            submatches: vec![(3, 7)],
        }
    }

    /// One batch of hits, under the run the overlay is waiting for.
    fn grep_chunk(state: &mut AppState, hits: &[grep::GrepMatch]) -> Vec<Effect> {
        let generation = state.finder.grep.generation;
        crate::app::reduce::reduce(
            state,
            Action::GrepChunk {
                generation,
                matches: hits.to_vec(),
            },
        )
    }

    /// The rows the grep list shows, as `path:line` for each of them.
    fn grepped(state: &AppState) -> Vec<String> {
        state
            .finder
            .shown
            .iter()
            .map(|index| {
                let hit = &state.finder.grep.matches[*index as usize];
                format!("{}:{}", hit.rel.to_string_lossy(), hit.line)
            })
            .collect()
    }

    #[test]
    fn the_search_waits_for_two_characters_and_asks_for_every_one_after_that() {
        let mut state = state();
        let effects = crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        assert_eq!(state.overlay, Overlay::Grep);
        assert_eq!(
            effects,
            Vec::new(),
            "nothing is asked for until there is something to ask with (§7)"
        );

        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('a')));
        assert!(
            crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('b')))
                .contains(&Effect::StartGrep {
                    generation: state.finder.grep.generation,
                    pattern: "ab".to_owned(),
                }),
            "two characters are a search: {:?}",
            state.finder.query_text()
        );
        assert!(state.finder.grep.running, "the list says it is going");

        // One character less, and the run that was going is not asked for.
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderDeleteBack));
        assert!(state.finder.grep.matches.is_empty());
        assert!(!state.finder.grep.running);
    }

    #[test]
    fn hits_land_in_the_order_the_run_gave_them_and_the_first_ones_replace() {
        let mut state = state();
        crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('a')));
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('b')));
        let generation = state.finder.grep.generation;

        // A batch from a query that has been typed over belongs to nobody (I3).
        crate::app::reduce::reduce(
            &mut state,
            Action::GrepChunk {
                generation: generation - 1,
                matches: vec![hit("stale.txt", 1)],
            },
        );
        assert!(state.finder.shown.is_empty(), "a stale batch was taken");

        grep_chunk(&mut state, &[hit("b.rs", 2), hit("a.rs", 1)]);
        assert_eq!(grepped(&state), ["b.rs:2", "a.rs:1"], "rg's own order");
        grep_chunk(&mut state, &[hit("c.rs", 3)]);
        assert_eq!(
            grepped(&state),
            ["b.rs:2", "a.rs:1", "c.rs:3"],
            "a later batch joins the same run"
        );

        crate::app::reduce::reduce(
            &mut state,
            Action::GrepDone {
                generation,
                truncated: true,
            },
        );
        assert!(
            !state.finder.grep.running,
            "the list stops saying it is going"
        );
        assert!(
            state.finder.grep.truncated,
            "the limit is what it stopped at"
        );

        // The next query starts over rather than adding to this list.
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('c')));
        grep_chunk(&mut state, &[hit("c.rs", 3)]);
        assert_eq!(grepped(&state), ["c.rs:3"], "the first batch replaces");
        assert_eq!(
            state.finder.grep.matches.len(),
            1,
            "the hits of the query before have nobody waiting for them"
        );
        assert!(
            !state.finder.grep.truncated,
            "the limit the last run stopped at is not this run's"
        );
    }

    #[test]
    fn a_pattern_rg_cannot_take_is_said_in_the_list() {
        let mut state = state();
        crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('(')));
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('a')));
        let generation = state.finder.grep.generation;
        grep_chunk(&mut state, &[hit("a.rs", 1)]);

        crate::app::reduce::reduce(
            &mut state,
            Action::GrepFailed {
                generation,
                message: "regex parse error: unclosed group".to_owned(),
            },
        );
        assert_eq!(
            state.finder.grep.error.as_deref(),
            Some("regex parse error: unclosed group")
        );
        assert!(state.finder.shown.is_empty(), "the rows are the old answer");
        assert!(!state.finder.grep.running);
    }

    #[test]
    fn without_ripgrep_the_overlay_comes_down_and_says_why() {
        let mut state = state();
        crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        let generation = state.finder.grep.generation;
        crate::app::reduce::reduce(
            &mut state,
            Action::GrepMissing {
                generation,
                message: "ripgrep (`rg`) is not installed".to_owned(),
            },
        );
        assert_eq!(state.overlay, Overlay::None, "§13.3");
        assert!(
            state
                .visible_message()
                .is_some_and(|message| message.text.contains("ripgrep")),
            "the status bar says why"
        );

        // The overlay will not open again: it would only come down again.
        crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        assert_eq!(state.overlay, Overlay::None);
        assert!(
            state
                .visible_message()
                .is_some_and(|message| message.text.contains("ripgrep")),
            "it says why instead of opening"
        );
    }

    #[test]
    fn ctrl_g_is_the_way_between_the_two_lists() {
        let mut state = state();
        crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        assert_eq!(state.overlay, Overlay::Grep);

        // Inside the overlay the same key is the switch, and a walk starts
        // under the file list that comes back.
        let effects = crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        assert_eq!(state.overlay, Overlay::Files);
        assert!(
            effects.contains(&Effect::ScanFiles {
                generation: state.finder.scan_gen
            }),
            "{effects:?}"
        );

        crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        assert_eq!(state.overlay, Overlay::Grep);
    }

    #[test]
    fn enter_on_a_hit_takes_the_reader_to_the_line_it_was_on() {
        let mut state = state();
        listed_root(&mut state);
        crate::app::reduce::reduce(&mut state, Action::Command(Command::OpenGrep));
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('a')));
        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderInsert('b')));
        grep_chunk(&mut state, &[hit("src/main.rs", 7)]);

        crate::app::reduce::reduce(&mut state, Action::Command(Command::FinderAccept));
        assert_eq!(state.overlay, Overlay::None);
        assert_eq!(state.focus, Focus::Preview, "the reader lands in the pane");
        assert_eq!(
            state.preview.path.as_deref(),
            Some(Path::new("src/main.rs")),
            "the file of the hit is what the preview is showing"
        );
        let focus = state.preview.focus.as_ref().expect("the line is marked");
        assert_eq!(focus.rel, Path::new("src/main.rs"));
        assert_eq!(focus.line, 7);
        assert_eq!(
            focus.ranges,
            [(3, 7)],
            "where on the line the hit lands, as the pane draws it"
        );
        assert!(
            state.preview.marks().is_some(),
            "the mark is on the file it belongs to"
        );
    }
}
