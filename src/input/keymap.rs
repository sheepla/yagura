//! Key to command resolution.
//!
//! The keymap is data, not code: state logic never sees a key code, so a
//! rebinding is a change to one table and the help overlay can be generated
//! from the very same table later on.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::command::Command;

/// Where a key press applies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Context {
    Tree,
    Preview,
    Finder,
    Watch,
    Help,
}

/// One row of the keymap.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    /// Contexts the binding applies to.
    pub contexts: &'static [Context],
    pub code: KeyCode,
    /// Modifiers that must be present. `SHIFT` is never listed: it is dropped
    /// before matching, because terminals report an uppercase letter as the
    /// uppercase character plus `SHIFT`.
    pub mods: KeyModifiers,
    pub command: Command,
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const CONTROL: KeyModifiers = KeyModifiers::CONTROL;

const BOTH: &[Context] = &[Context::Tree, Context::Preview];
const TREE: &[Context] = &[Context::Tree];
const PREVIEW: &[Context] = &[Context::Preview];

macro_rules! binding {
    ($contexts:ident, $code:expr, $mods:expr, $command:ident) => {
        Binding {
            contexts: $contexts,
            code: $code,
            mods: $mods,
            command: Command::$command,
        }
    };
}

/// The whole keymap.
pub const BINDINGS: &[Binding] = &[
    // Quit.
    binding!(BOTH, KeyCode::Char('q'), NONE, Quit),
    binding!(BOTH, KeyCode::Char('c'), CONTROL, Quit),
    // Shell level commands.
    binding!(BOTH, KeyCode::Tab, NONE, ToggleFocus),
    binding!(BOTH, KeyCode::Esc, NONE, Back),
    binding!(BOTH, KeyCode::Char('t'), NONE, ToggleTree),
    binding!(BOTH, KeyCode::Char('s'), NONE, CyclePreviewMode),
    binding!(BOTH, KeyCode::Char('B'), NONE, ToggleBaseline),
    binding!(BOTH, KeyCode::Char('F'), NONE, ToggleFollow),
    binding!(BOTH, KeyCode::Char('c'), NONE, ToggleChangedOnly),
    binding!(BOTH, KeyCode::Char('o'), NONE, CycleSort),
    binding!(BOTH, KeyCode::Char('r'), NONE, Reload),
    // Hunk navigation: only a diff has hunks, so the reducer ignores these
    // outside the diff modes.
    binding!(BOTH, KeyCode::Char('n'), NONE, NextHunk),
    binding!(BOTH, KeyCode::Char('N'), NONE, PrevHunk),
    // Tree navigation.
    binding!(TREE, KeyCode::Char('j'), NONE, MoveDown),
    binding!(TREE, KeyCode::Char('k'), NONE, MoveUp),
    binding!(TREE, KeyCode::Char('g'), NONE, First),
    binding!(TREE, KeyCode::Char('G'), NONE, Last),
    binding!(TREE, KeyCode::Char('d'), NONE, HalfPageDown),
    binding!(TREE, KeyCode::Char('u'), NONE, HalfPageUp),
    binding!(TREE, KeyCode::Char('f'), NONE, PageDown),
    binding!(TREE, KeyCode::Char('b'), NONE, PageUp),
    binding!(TREE, KeyCode::Char('h'), NONE, Collapse),
    binding!(TREE, KeyCode::Char('l'), NONE, Expand),
    binding!(TREE, KeyCode::Enter, NONE, OpenPreview),
    // Preview scrolling.
    binding!(PREVIEW, KeyCode::Char('j'), NONE, ScrollDown),
    binding!(PREVIEW, KeyCode::Char('k'), NONE, ScrollUp),
    binding!(PREVIEW, KeyCode::Char('g'), NONE, ScrollTop),
    binding!(PREVIEW, KeyCode::Char('G'), NONE, ScrollBottom),
    binding!(PREVIEW, KeyCode::Char('d'), NONE, ScrollHalfPageDown),
    binding!(PREVIEW, KeyCode::Char('u'), NONE, ScrollHalfPageUp),
    binding!(PREVIEW, KeyCode::Char('f'), NONE, ScrollPageDown),
    binding!(PREVIEW, KeyCode::Char('b'), NONE, ScrollPageUp),
    binding!(PREVIEW, KeyCode::Char(' '), NONE, ScrollPageDown),
    // Sideways scrolling: the preview cuts instead of wrapping.
    binding!(PREVIEW, KeyCode::Left, NONE, ScrollLeft),
    binding!(PREVIEW, KeyCode::Right, NONE, ScrollRight),
    binding!(PREVIEW, KeyCode::Char('H'), NONE, ScrollLeft),
    binding!(PREVIEW, KeyCode::Char('L'), NONE, ScrollRight),
    binding!(PREVIEW, KeyCode::Char('0'), NONE, ScrollPageLeft),
    binding!(PREVIEW, KeyCode::Char('$'), NONE, ScrollPageRight),
];

/// Resolves a key press in a context.
pub fn command_for(context: Context, key: KeyEvent) -> Option<Command> {
    let mods = key.modifiers.difference(KeyModifiers::SHIFT);
    BINDINGS
        .iter()
        .find(|binding| {
            binding.contexts.contains(&context) && binding.code == key.code && binding.mods == mods
        })
        .map(|binding| binding.command)
}

/// Every binding that applies to a context.
pub fn bindings_for(context: Context) -> impl Iterator<Item = &'static Binding> {
    BINDINGS
        .iter()
        .filter(move |binding| binding.contexts.contains(&context))
}

/// Renders a binding the way the help overlay will show it.
pub fn describe(binding: &Binding) -> String {
    let mut key = String::new();
    if binding.mods.contains(KeyModifiers::CONTROL) {
        key.push_str("Ctrl+");
    }
    let name = match binding.code {
        KeyCode::Char(' ') => "Space".to_owned(),
        KeyCode::Char(letter) => letter.to_uppercase().to_string(),
        KeyCode::Enter => "Enter".to_owned(),
        KeyCode::Esc => "Esc".to_owned(),
        KeyCode::Tab => "Tab".to_owned(),
        other => format!("{other:?}"),
    };
    key.push_str(&name);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn no_context_has_two_commands_for_the_same_key() {
        for context in [
            Context::Tree,
            Context::Preview,
            Context::Finder,
            Context::Watch,
            Context::Help,
        ] {
            let mut seen: Vec<(KeyCode, KeyModifiers)> = BINDINGS
                .iter()
                .filter(|binding| binding.contexts.contains(&context))
                .map(|binding| (binding.code, binding.mods))
                .collect();
            let count = seen.len();
            seen.sort_by_key(|(code, mods)| (format!("{code:?}"), format!("{mods:?}")));
            seen.dedup();
            assert_eq!(seen.len(), count, "{context:?} has a duplicated binding");
        }
    }

    #[test]
    fn upper_case_letters_work_with_and_without_shift() {
        assert_eq!(
            command_for(Context::Tree, press(KeyCode::Char('G'))),
            Some(Command::Last)
        );
        let mut with_shift = press(KeyCode::Char('G'));
        with_shift.modifiers = KeyModifiers::SHIFT;
        assert_eq!(command_for(Context::Tree, with_shift), Some(Command::Last));
    }

    #[test]
    fn plain_and_control_keys_are_not_confused() {
        assert_eq!(
            command_for(Context::Tree, press(KeyCode::Char('c'))),
            Some(Command::ToggleChangedOnly)
        );
        assert_eq!(
            command_for(Context::Tree, ctrl(KeyCode::Char('c'))),
            Some(Command::Quit)
        );
    }

    #[test]
    fn follow_changed_only_and_order_are_bound_in_both_panes() {
        for context in [Context::Tree, Context::Preview] {
            assert_eq!(
                command_for(context, press(KeyCode::Char('F'))),
                Some(Command::ToggleFollow),
                "{context:?} F"
            );
            assert_eq!(
                command_for(context, press(KeyCode::Char('c'))),
                Some(Command::ToggleChangedOnly),
                "{context:?} c"
            );
            assert_eq!(
                command_for(context, press(KeyCode::Char('o'))),
                Some(Command::CycleSort),
                "{context:?} o"
            );
        }
    }

    #[test]
    fn the_same_key_can_mean_different_things_per_context() {
        assert_eq!(
            command_for(Context::Tree, press(KeyCode::Char('j'))),
            Some(Command::MoveDown)
        );
        assert_eq!(
            command_for(Context::Preview, press(KeyCode::Char('j'))),
            Some(Command::ScrollDown)
        );
        assert_eq!(command_for(Context::Tree, press(KeyCode::Char(' '))), None);
        assert_eq!(
            command_for(Context::Preview, press(KeyCode::Char(' '))),
            Some(Command::ScrollPageDown)
        );
    }

    #[test]
    fn quit_works_from_both_panes() {
        for context in [Context::Tree, Context::Preview] {
            assert_eq!(
                command_for(context, press(KeyCode::Char('q'))),
                Some(Command::Quit)
            );
        }
    }

    #[test]
    fn unbound_keys_resolve_to_nothing() {
        for context in [
            Context::Tree,
            Context::Preview,
            Context::Watch,
            Context::Help,
        ] {
            assert_eq!(command_for(context, press(KeyCode::F(5))), None);
            assert_eq!(command_for(context, press(KeyCode::Char('Z'))), None);
        }
    }

    #[test]
    fn every_context_resolves_its_specified_keys() {
        let expected: &[(Context, KeyCode, Option<Command>)] = &[
            (Context::Tree, KeyCode::Char('j'), Some(Command::MoveDown)),
            (Context::Tree, KeyCode::Char('k'), Some(Command::MoveUp)),
            (Context::Tree, KeyCode::Char('h'), Some(Command::Collapse)),
            (Context::Tree, KeyCode::Char('l'), Some(Command::Expand)),
            (Context::Tree, KeyCode::Enter, Some(Command::OpenPreview)),
            (Context::Tree, KeyCode::Char('g'), Some(Command::First)),
            (Context::Tree, KeyCode::Char('G'), Some(Command::Last)),
            (
                Context::Tree,
                KeyCode::Char('d'),
                Some(Command::HalfPageDown),
            ),
            (Context::Tree, KeyCode::Char('u'), Some(Command::HalfPageUp)),
            (Context::Tree, KeyCode::Char('f'), Some(Command::PageDown)),
            (Context::Tree, KeyCode::Char('b'), Some(Command::PageUp)),
            (Context::Tree, KeyCode::Char('t'), Some(Command::ToggleTree)),
            (
                Context::Tree,
                KeyCode::Char('s'),
                Some(Command::CyclePreviewMode),
            ),
            (
                Context::Tree,
                KeyCode::Char('B'),
                Some(Command::ToggleBaseline),
            ),
            (
                Context::Tree,
                KeyCode::Char('F'),
                Some(Command::ToggleFollow),
            ),
            (
                Context::Tree,
                KeyCode::Char('c'),
                Some(Command::ToggleChangedOnly),
            ),
            (Context::Tree, KeyCode::Char('o'), Some(Command::CycleSort)),
            (
                Context::Preview,
                KeyCode::Char('B'),
                Some(Command::ToggleBaseline),
            ),
            (Context::Tree, KeyCode::Tab, Some(Command::ToggleFocus)),
            (Context::Tree, KeyCode::Esc, Some(Command::Back)),
            (Context::Tree, KeyCode::Char('r'), Some(Command::Reload)),
            (Context::Preview, KeyCode::Char('r'), Some(Command::Reload)),
            (Context::Tree, KeyCode::Char('n'), Some(Command::NextHunk)),
            (
                Context::Preview,
                KeyCode::Char('n'),
                Some(Command::NextHunk),
            ),
            (Context::Tree, KeyCode::Char('N'), Some(Command::PrevHunk)),
            (
                Context::Preview,
                KeyCode::Char('N'),
                Some(Command::PrevHunk),
            ),
            (
                Context::Preview,
                KeyCode::Char('j'),
                Some(Command::ScrollDown),
            ),
            (
                Context::Preview,
                KeyCode::Char('k'),
                Some(Command::ScrollUp),
            ),
            (
                Context::Preview,
                KeyCode::Char('f'),
                Some(Command::ScrollPageDown),
            ),
            (
                Context::Preview,
                KeyCode::Char(' '),
                Some(Command::ScrollPageDown),
            ),
            (
                Context::Preview,
                KeyCode::Char('b'),
                Some(Command::ScrollPageUp),
            ),
            (
                Context::Preview,
                KeyCode::Char('d'),
                Some(Command::ScrollHalfPageDown),
            ),
            (
                Context::Preview,
                KeyCode::Char('u'),
                Some(Command::ScrollHalfPageUp),
            ),
            (
                Context::Preview,
                KeyCode::Char('g'),
                Some(Command::ScrollTop),
            ),
            (
                Context::Preview,
                KeyCode::Char('G'),
                Some(Command::ScrollBottom),
            ),
            (Context::Preview, KeyCode::Left, Some(Command::ScrollLeft)),
            (Context::Preview, KeyCode::Right, Some(Command::ScrollRight)),
            (
                Context::Preview,
                KeyCode::Char('H'),
                Some(Command::ScrollLeft),
            ),
            (
                Context::Preview,
                KeyCode::Char('L'),
                Some(Command::ScrollRight),
            ),
            (
                Context::Preview,
                KeyCode::Char('0'),
                Some(Command::ScrollPageLeft),
            ),
            (
                Context::Preview,
                KeyCode::Char('$'),
                Some(Command::ScrollPageRight),
            ),
        ];
        for (context, code, command) in expected {
            assert_eq!(
                command_for(*context, press(*code)),
                *command,
                "{context:?} {code:?}"
            );
        }
    }

    #[test]
    fn sideways_scrolling_belongs_to_the_preview_alone() {
        // `h` and `l` fold a directory in the tree, so the preview scrolls
        // sideways with the arrow keys and with the shifted letters instead.
        for code in [KeyCode::Left, KeyCode::Right, KeyCode::Char('0')] {
            assert_eq!(command_for(Context::Tree, press(code)), None, "{code:?}");
            assert!(
                command_for(Context::Preview, press(code)).is_some(),
                "{code:?}"
            );
        }
        assert_eq!(command_for(Context::Tree, press(KeyCode::Char('H'))), None);
        assert_eq!(
            command_for(Context::Preview, press(KeyCode::Char('H'))),
            Some(Command::ScrollLeft)
        );
    }

    #[test]
    fn bindings_describe_themselves_for_the_help_overlay() {
        let quit = bindings_for(Context::Tree)
            .find(|binding| binding.command == Command::Quit && binding.code == KeyCode::Char('q'))
            .expect("q binding");
        assert_eq!(describe(quit), "Q");
        let control = bindings_for(Context::Tree)
            .find(|binding| binding.mods.contains(KeyModifiers::CONTROL))
            .expect("ctrl binding");
        assert_eq!(describe(control), "Ctrl+C");
        let enter = bindings_for(Context::Tree)
            .find(|binding| binding.code == KeyCode::Enter)
            .expect("enter binding");
        assert_eq!(describe(enter), "Enter");
    }
}
