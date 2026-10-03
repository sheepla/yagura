//! The help reducer: opening the list of keys and moving through it.

use ratatui::layout::Rect;

use crate::app::action::Action;
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::{AppState, Overlay};
use crate::features::help::view;
use crate::ui::layout;

/// Whether a command belongs to the help overlay rather than to the panes
/// behind it.
pub fn owns(command: Command) -> bool {
    matches!(
        command,
        Command::OpenHelp
            | Command::HelpDown
            | Command::HelpUp
            | Command::HelpPageDown
            | Command::HelpPageUp
            | Command::HelpTop
            | Command::HelpBottom
    )
}

/// Applies one action that concerns the help overlay.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Action::Command(command) = action else {
        return Vec::new();
    };
    match command {
        Command::OpenHelp => {
            state.overlay = Overlay::Help;
            state.help.open();
        }
        Command::HelpDown => scroll(state, 1),
        Command::HelpUp => scroll(state, -1),
        Command::HelpPageDown => scroll(state, page(state)),
        Command::HelpPageUp => scroll(state, -page(state)),
        Command::HelpTop => state.help.scroll_to(0, last(state)),
        Command::HelpBottom => {
            let last = last(state);
            state.help.scroll_to(last, last);
        }
        _ => {}
    }
    Vec::new()
}

fn scroll(state: &mut AppState, by: isize) {
    let last = last(state);
    state.help.scroll_by(by, last);
}

/// One page is what the popup shows at once.
fn page(state: &AppState) -> isize {
    rows(state) as isize
}

/// How far down the list the reader can go: the whole of it, less what the
/// popup has room for.
fn last(state: &AppState) -> usize {
    view::content_height(state).saturating_sub(rows(state))
}

fn rows(state: &AppState) -> usize {
    layout::help_rows(area(state)) as usize
}

fn area(state: &AppState) -> Rect {
    Rect::new(0, 0, state.size.0, state.size.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(width: u16, height: u16) -> AppState {
        let config = crate::app::state::Config {
            root: std::path::PathBuf::from("/root"),
            icons: crate::cli::IconStyle::Text,
            baseline: crate::cli::BaselineKind::Auto,
            ignore_enabled: true,
            no_color: false,
            log: None,
            selftest_panic: false,
        };
        AppState::new(config, (width, height))
    }

    fn press(state: &mut AppState, command: Command) {
        reduce(state, &Action::Command(command));
    }

    #[test]
    fn the_key_that_opens_the_overlay_is_the_one_that_is_shown() {
        let mut state = state(80, 24);
        press(&mut state, Command::OpenHelp);
        assert_eq!(state.overlay, Overlay::Help);
        assert_eq!(state.help.scroll, 0, "an overlay opens at the top");
    }

    #[test]
    fn the_reader_walks_the_list_and_stops_at_both_ends() {
        let mut state = state(80, 24);
        press(&mut state, Command::OpenHelp);
        let last = last(&state);
        assert!(last > 0, "the list is longer than the pane shows");

        press(&mut state, Command::HelpDown);
        assert_eq!(state.help.scroll, 1);
        press(&mut state, Command::HelpUp);
        assert_eq!(state.help.scroll, 0, "nothing above the first line");

        press(&mut state, Command::HelpBottom);
        assert_eq!(state.help.scroll, last, "the far end of the list");
        press(&mut state, Command::HelpDown);
        assert_eq!(state.help.scroll, last, "nothing below the last line");

        press(&mut state, Command::HelpTop);
        assert_eq!(state.help.scroll, 0);
    }

    #[test]
    fn a_page_is_what_the_popup_shows_at_once() {
        let mut state = state(80, 24);
        press(&mut state, Command::OpenHelp);
        let page = rows(&state);
        press(&mut state, Command::HelpPageDown);
        assert_eq!(state.help.scroll, page);
        press(&mut state, Command::HelpPageUp);
        assert_eq!(state.help.scroll, 0);
    }

    #[test]
    fn only_the_help_overlay_takes_the_help_commands() {
        assert!(owns(Command::OpenHelp));
        assert!(owns(Command::HelpDown));
        assert!(!owns(Command::OpenWatch), "the watch list has its own");
        assert!(!owns(Command::ScrollDown), "the panes keep their own");
    }
}
