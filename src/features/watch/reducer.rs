//! The watch reducer: opening the log, walking it, and jumping from it (§9).

use std::time::SystemTime;

use ratatui::layout::Rect;

use crate::app::action::Action;
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::{AppState, Focus, Overlay};
use crate::core::fswatch::FsChange;
use crate::features::tree;
use crate::ui::layout;

/// Whether a command belongs to the watch list rather than to the panes
/// behind it.
pub fn owns(command: Command) -> bool {
    matches!(
        command,
        Command::OpenWatch
            | Command::WatchDown
            | Command::WatchUp
            | Command::WatchTop
            | Command::WatchBottom
            | Command::WatchAccept
    )
}

/// The log takes a batch of changes whether or not its overlay is up (§12).
pub fn record(state: &mut AppState, changes: &[FsChange], at: SystemTime) {
    state.watch.record(changes, at);
    if state.overlay == Overlay::Watch {
        settle(state);
    }
}

/// Applies one action that concerns the watch list.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Action::Command(command) = action else {
        return Vec::new();
    };
    match command {
        Command::OpenWatch => {
            state.overlay = Overlay::Watch;
            state.watch.open();
            settle(state);
        }
        Command::WatchDown => {
            state.watch.select_by(1);
            settle(state);
        }
        Command::WatchUp => {
            state.watch.select_by(-1);
            settle(state);
        }
        Command::WatchTop => {
            state.watch.select_first();
            settle(state);
        }
        Command::WatchBottom => {
            state.watch.select_last();
            settle(state);
        }
        Command::WatchAccept => return accept(state),
        _ => {}
    }
    Vec::new()
}

/// The selection is kept on screen after every move, the way the tree does
/// (§11).
fn settle(state: &mut AppState) {
    let viewport = layout::watch_rows(area(state)) as usize;
    state.watch.scroll_into_view(viewport);
}

/// `Enter`: the overlay comes down and the tree walks to the path of the row,
/// whether the file is still there or not (§9).
fn accept(state: &mut AppState) -> Vec<Effect> {
    let Some(entry) = state.watch.selected().cloned() else {
        return Vec::new();
    };
    state.overlay = Overlay::None;
    state.focus = Focus::Preview;
    let mut effects = Vec::new();
    tree::reducer::jump_to(state, entry.rel, &mut effects);
    effects
}

fn area(state: &AppState) -> Rect {
    Rect::new(0, 0, state.size.0, state.size.1)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::app::state::Config;
    use crate::cli::{BaselineKind, IconStyle};
    use crate::core::fsmodel::{DirEntry, NodeKind};
    use crate::core::fswatch::ChangeKind;

    fn state(width: u16, height: u16) -> AppState {
        let config = Config {
            root: PathBuf::from("/root"),
            icons: IconStyle::Text,
            baseline: BaselineKind::Auto,
            ignore_enabled: true,
            no_color: false,
            log: None,
            selftest_panic: false,
        };
        AppState::new(config, (width, height))
    }

    fn change(rel: &str, kind: ChangeKind) -> FsChange {
        FsChange {
            rel: PathBuf::from(rel),
            kind,
        }
    }

    fn press(state: &mut AppState, command: Command) {
        reduce(state, &Action::Command(command));
    }

    fn entry(name: &str, kind: NodeKind) -> DirEntry {
        DirEntry {
            name: name.into(),
            kind,
            link_target: None,
            unreadable: false,
        }
    }

    /// The root listing in hand, with a `src` directory below it whose own
    /// listing the tree has not asked for yet.
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
    fn the_log_is_recorded_while_the_overlay_is_down() {
        let mut state = state(80, 24);
        record(
            &mut state,
            &[change("a.txt", ChangeKind::Modify)],
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(state.overlay, Overlay::None, "the overlay was never opened");
        assert_eq!(state.watch.len(), 1, "the change was kept for later");
    }

    #[test]
    fn the_key_that_opens_the_log_shows_what_it_has_kept() {
        let mut state = state(80, 24);
        record(
            &mut state,
            &[change("a.txt", ChangeKind::Modify)],
            SystemTime::UNIX_EPOCH,
        );
        press(&mut state, Command::OpenWatch);
        assert_eq!(state.overlay, Overlay::Watch);
        assert!(state.watch.following(), "the newest entry is on screen");
        assert_eq!(
            state.watch.selected().expect("the entry").rel,
            PathBuf::from("a.txt")
        );
    }

    #[test]
    fn moving_up_leaves_the_following_and_back_to_the_end_returns_to_it() {
        let mut state = state(80, 24);
        record(
            &mut state,
            &[
                change("a.txt", ChangeKind::Modify),
                change("b.txt", ChangeKind::Create),
            ],
            SystemTime::UNIX_EPOCH,
        );
        press(&mut state, Command::OpenWatch);
        press(&mut state, Command::WatchUp);
        assert!(!state.watch.following());
        record(
            &mut state,
            &[change("c.txt", ChangeKind::Remove)],
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(state.watch.unread(), 1, "one arrived while looking away");

        press(&mut state, Command::WatchBottom);
        assert!(state.watch.following());
        assert_eq!(state.watch.unread(), 0);
        assert_eq!(
            state.watch.selected().expect("the entry").rel,
            PathBuf::from("c.txt")
        );
    }

    #[test]
    fn enter_takes_the_row_to_the_tree_and_closes_the_overlay() {
        let mut state = state(80, 24);
        listed_root(&mut state);
        record(
            &mut state,
            &[change("src/main.rs", ChangeKind::Modify)],
            SystemTime::UNIX_EPOCH,
        );
        press(&mut state, Command::OpenWatch);
        let effects = reduce(&mut state, &Action::Command(Command::WatchAccept));
        assert_eq!(state.overlay, Overlay::None, "the panes are showing again");
        assert_eq!(state.focus, Focus::Preview, "the reader lands in the pane");
        assert_eq!(
            state.tree.jump,
            Some(PathBuf::from("src/main.rs")),
            "the jump waits for the listing that stands in its way"
        );
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::LoadDir { rel, .. } if rel == Path::new("src")
            )),
            "the tree was asked to walk there: {effects:?}"
        );
    }

    #[test]
    fn only_the_watch_overlay_takes_the_watch_commands() {
        assert!(owns(Command::OpenWatch));
        assert!(owns(Command::WatchAccept));
        assert!(!owns(Command::OpenHelp), "the help list has its own");
        assert!(!owns(Command::MoveDown), "the tree keeps its own");
    }
}
