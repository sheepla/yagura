//! The root reducer: the one place where actions meet the state.
//!
//! Every function here is pure: it reads the state, writes the state and
//! returns the work to do next. It never touches the filesystem, the clock or
//! a channel (invariants I1 and I2). Time arrives inside the actions.

use crate::app::action::Action;
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::AppState;
use crate::features::{preview, tree};
use crate::input::keymap;

/// Applies one action and reports the effects it caused.
pub fn reduce(state: &mut AppState, action: Action) -> Vec<Effect> {
    if matches!(action, Action::Key(_)) {
        // An error stays on the status bar until the user does something.
        state.clear_sticky_message();
    }
    let Some(action) = resolve(state, action) else {
        return Vec::new();
    };
    // The result of a load owns its document, so it is taken by value and
    // never cloned: a document is up to a mebibyte of lines.
    if let Action::PreviewLoaded {
        rel,
        generation,
        result,
    } = action
    {
        return preview::reducer::on_loaded(state, rel, generation, result);
    }
    match &action {
        Action::Resize { width, height } => {
            state.size = (*width, *height);
            tree::reducer::sync_viewport(state);
            sync_preview_viewport(state);
            state.tree.refresh();
            Vec::new()
        }
        Action::Tick { now } => {
            state.now = *now;
            Vec::new()
        }
        Action::Key(_) | Action::Quit => {
            if matches!(action, Action::Quit) {
                state.quitting = true;
            }
            Vec::new()
        }
        Action::Paste(_)
        | Action::Command(_)
        | Action::DirLoaded { .. }
        | Action::PreviewLoaded { .. } => {
            // The tree owns the selection, the preview owns the scroll, and the
            // selection decides what the preview should be showing.
            let mut effects = Vec::new();
            if let Action::Command(command) = &action {
                if preview_scrolls(*command) {
                    effects.extend(preview::reducer::reduce(state, &action));
                } else {
                    effects.extend(tree::reducer::reduce(state, &action));
                }
            } else {
                effects.extend(tree::reducer::reduce(state, &action));
            }
            effects.extend(preview::reducer::sync_target(state));
            effects
        }
    }
}

/// Commands the preview pane handles itself.
fn preview_scrolls(command: Command) -> bool {
    matches!(
        command,
        Command::ScrollDown
            | Command::ScrollUp
            | Command::ScrollPageDown
            | Command::ScrollPageUp
            | Command::ScrollHalfPageDown
            | Command::ScrollHalfPageUp
            | Command::ScrollTop
            | Command::ScrollBottom
            | Command::ScrollLeft
            | Command::ScrollRight
            | Command::ScrollPageLeft
            | Command::ScrollPageRight
            | Command::Reload
    )
}

/// Gives the preview pane its height from the current layout.
pub fn sync_preview_viewport(state: &mut AppState) {
    state.preview.viewport = state.preview_viewport_height();
    state.preview.clamp();
}

/// Turns raw input into a command using the keymap for the current context.
/// `None` means the key is not bound and the action is dropped.
fn resolve(state: &AppState, action: Action) -> Option<Action> {
    match action {
        Action::Key(key) => keymap::command_for(state.context(), key).map(Action::Command),
        other => Some(other),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::*;
    use crate::app::command::Command;
    use crate::app::state::{Config, Focus, PreviewMode};
    use crate::cli::{BaselineKind, IconStyle};

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

    fn key(code: KeyCode) -> Action {
        Action::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// A projection of everything a reducer may change, used to show that
    /// reducing the same actions twice produces the same state.
    fn fingerprint(state: &AppState) -> String {
        format!(
            "focus={:?} mode={:?} overlay={:?} tree_visible={} follow={} quit={} sel={:?} scroll={} rows={:?} msg={:?}",
            state.focus,
            state.preview_mode,
            state.overlay,
            state.tree_visible,
            state.follow,
            state.quitting,
            state.tree.selected,
            state.tree.scroll,
            state.tree.visible,
            state.visible_message().map(|message| message.text.clone()),
        )
    }

    #[test]
    fn key_presses_are_resolved_into_commands() {
        let mut state = state();
        reduce(&mut state, key(KeyCode::Char('t')));
        assert!(!state.tree_visible, "`t` did not hide the tree");
        reduce(&mut state, key(KeyCode::Char('s')));
        assert_eq!(state.preview_mode, PreviewMode::InlineDiff);
        reduce(&mut state, key(KeyCode::Tab));
        assert_eq!(state.focus, Focus::Preview);
    }

    #[test]
    fn an_unbound_key_changes_nothing() {
        let mut state = state();
        let before = fingerprint(&state);
        reduce(&mut state, key(KeyCode::F(12)));
        assert_eq!(fingerprint(&state), before);
    }

    #[test]
    fn quit_is_sticky() {
        let mut state = state();
        reduce(&mut state, Action::Quit);
        assert!(state.quitting);
    }

    #[test]
    fn resize_updates_the_viewport_and_scroll_offset() {
        let mut state = state();
        state.tree.set_viewport(2);
        state.tree.select_last();
        let scroll_before = state.tree.scroll;
        reduce(
            &mut state,
            Action::Resize {
                width: 80,
                height: 60,
            },
        );
        assert_eq!(state.size, (80, 60));
        assert_eq!(state.tree.viewport, 57);
        assert!(state.tree.scroll <= scroll_before);
    }

    #[test]
    fn ticks_only_move_the_clock() {
        let mut state = state();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        let effects = reduce(&mut state, Action::Tick { now });
        assert_eq!(state.now, now);
        assert!(effects.is_empty());
    }

    #[test]
    fn the_reducer_is_deterministic() {
        // Built twice because an action carries results it cannot be cloned
        // from, and the point of the test is to replay the very same input.
        fn actions() -> Vec<Action> {
            vec![
                Action::Resize {
                    width: 100,
                    height: 30,
                },
                key(KeyCode::Char('s')),
                Action::Command(Command::MoveDown),
                Action::Command(Command::MoveDown),
                Action::Command(Command::Expand),
                key(KeyCode::Tab),
                Action::Command(Command::ScrollDown),
                Action::Command(Command::Back),
            ]
        }
        let mut first = state();
        for action in actions() {
            reduce(&mut first, action);
        }
        let mut second = state();
        for action in actions() {
            reduce(&mut second, action);
        }
        assert_eq!(fingerprint(&first), fingerprint(&second));
    }

    #[test]
    fn key_repeats_are_treated_like_presses() {
        let mut pressed = state();
        reduce(&mut pressed, key(KeyCode::Char('s')));
        let mut repeated = state();
        let mut event = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        event.kind = KeyEventKind::Repeat;
        reduce(&mut repeated, Action::Key(event));
        assert_eq!(first(&pressed), first(&repeated));
    }

    fn first(state: &AppState) -> PreviewMode {
        state.preview_mode
    }

    #[test]
    fn tree_navigation_breaks_follow_mode() {
        let mut state = state();
        state.follow = true;
        reduce(&mut state, Action::Command(Command::MoveDown));
        assert!(!state.follow);
    }

    #[test]
    fn navigation_keys_only_act_on_the_focused_pane() {
        let mut state = state();
        state.focus = Focus::Preview;
        let before = fingerprint(&state);
        reduce(&mut state, key(KeyCode::Char('j')));
        assert_eq!(fingerprint(&state), before);
    }
}
