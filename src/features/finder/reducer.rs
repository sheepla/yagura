//! The finder reducer: what an overlay does with what happens to it.

use std::path::PathBuf;

use ratatui_textarea::CursorMove;

use crate::app::action::Action;
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::{AppState, Focus, Overlay};
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
            query_changed(state);
            Vec::new()
        }
        Action::FilesFound { generation, rels } => on_found(state, *generation, rels),
        Action::FilesScanDone { generation } => {
            if *generation == state.finder.scan_gen {
                state.finder.scanning = false;
            }
            Vec::new()
        }
        Action::Tick { .. } => {
            refresh(state);
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn on_command(state: &mut AppState, command: Command) -> Vec<Effect> {
    match command {
        // The two commands that have work of their own to ask for.
        Command::OpenFiles => return open(state),
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
            query_changed(state);
        }
        Command::FinderCursorLeft => state.finder.query.move_cursor(CursorMove::Back),
        Command::FinderCursorRight => state.finder.query.move_cursor(CursorMove::Forward),
        Command::FinderCursorWordLeft => state.finder.query.move_cursor(CursorMove::WordBack),
        Command::FinderCursorWordRight => state.finder.query.move_cursor(CursorMove::WordForward),
        Command::FinderCursorHome => state.finder.query.move_cursor(CursorMove::Head),
        Command::FinderCursorEnd => state.finder.query.move_cursor(CursorMove::End),
        Command::FinderDeleteBack => state.finder.delete_char(),
        Command::FinderDeleteForward => state.finder.delete_next_char(),
        Command::FinderDeleteWord => state.finder.delete_word(),
        Command::FinderDeleteToStart => state.finder.delete_to_start(),
        Command::FinderDeleteToEnd => state.finder.delete_to_end(),
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

/// `Enter`: the row becomes the selection and the preview (§4).
///
/// With nothing in the list there is nowhere to go, and `Enter` does nothing
/// (§7).
fn accept(state: &mut AppState) -> Vec<Effect> {
    let Some(rel) = state.finder.selected_path() else {
        return Vec::new();
    };
    state.overlay = Overlay::None;
    crate::app::reduce::sync_preview_viewport(state);
    state.focus = Focus::Preview;
    let mut effects = Vec::new();
    tree::reducer::jump_to(state, rel, &mut effects);
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

/// The query text changed: the list starts again from the first row, so the
/// first row is always the best answer to what is typed now.
fn query_changed(state: &mut AppState) {
    state.finder.select_first();
    state.finder.scroll = 0;
    refresh(state);
}

/// Brings the list, the selection and the window over it in step.
fn refresh(state: &mut AppState) {
    state.finder.refresh(&state.baseline.changed);
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
}
