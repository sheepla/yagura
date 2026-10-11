//! Key to command resolution.
//!
//! The keymap is data, not code: state logic never sees a key code, so a
//! rebinding is a change to one table, and the help overlay is generated
//! from the very same table.

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
const ALT: KeyModifiers = KeyModifiers::ALT;

const BOTH: &[Context] = &[Context::Tree, Context::Preview];
const TREE: &[Context] = &[Context::Tree];
const PREVIEW: &[Context] = &[Context::Preview];
const FINDER: &[Context] = &[Context::Finder];
const WATCH: &[Context] = &[Context::Watch];
const HELP: &[Context] = &[Context::Help];

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
    // The other overlays and the editor (§5).
    binding!(BOTH, KeyCode::Char('w'), NONE, OpenWatch),
    binding!(BOTH, KeyCode::Char('?'), NONE, OpenHelp),
    binding!(BOTH, KeyCode::Char('e'), NONE, OpenEditor),
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
    // The finder overlays (§5): a plain character belongs to the query box,
    // so it never appears here and the reducer hands it over instead (§13.1).
    binding!(BOTH, KeyCode::Char('p'), CONTROL, OpenFiles),
    binding!(BOTH, KeyCode::Char('g'), CONTROL, OpenGrep),
    // Inside the overlay the two lists are switched between with the same
    // key that opened them (§13.3).
    binding!(FINDER, KeyCode::Char('g'), CONTROL, OpenGrep),
    // Line editing.
    binding!(FINDER, KeyCode::Left, NONE, FinderCursorLeft),
    binding!(FINDER, KeyCode::Right, NONE, FinderCursorRight),
    binding!(FINDER, KeyCode::Char('b'), CONTROL, FinderCursorLeft),
    binding!(FINDER, KeyCode::Char('f'), CONTROL, FinderCursorRight),
    binding!(FINDER, KeyCode::Char('b'), ALT, FinderCursorWordLeft),
    binding!(FINDER, KeyCode::Char('f'), ALT, FinderCursorWordRight),
    binding!(FINDER, KeyCode::Home, NONE, FinderCursorHome),
    binding!(FINDER, KeyCode::End, NONE, FinderCursorEnd),
    binding!(FINDER, KeyCode::Char('a'), CONTROL, FinderCursorHome),
    binding!(FINDER, KeyCode::Char('e'), CONTROL, FinderCursorEnd),
    binding!(FINDER, KeyCode::Backspace, NONE, FinderDeleteBack),
    binding!(FINDER, KeyCode::Char('h'), CONTROL, FinderDeleteBack),
    binding!(FINDER, KeyCode::Delete, NONE, FinderDeleteForward),
    binding!(FINDER, KeyCode::Char('d'), CONTROL, FinderDeleteForward),
    binding!(FINDER, KeyCode::Char('w'), CONTROL, FinderDeleteWord),
    binding!(FINDER, KeyCode::Char('u'), CONTROL, FinderDeleteToStart),
    binding!(FINDER, KeyCode::Char('k'), CONTROL, FinderDeleteToEnd),
    // The list.
    binding!(FINDER, KeyCode::Up, NONE, FinderSelectUp),
    binding!(FINDER, KeyCode::Down, NONE, FinderSelectDown),
    binding!(FINDER, KeyCode::Char('p'), CONTROL, FinderSelectUp),
    binding!(FINDER, KeyCode::Char('n'), CONTROL, FinderSelectDown),
    binding!(FINDER, KeyCode::PageUp, NONE, FinderPageUp),
    binding!(FINDER, KeyCode::PageDown, NONE, FinderPageDown),
    // The preview beside the list scrolls with the readline-ish `Alt+j/k`.
    binding!(FINDER, KeyCode::Char('j'), ALT, ScrollDown),
    binding!(FINDER, KeyCode::Char('k'), ALT, ScrollUp),
    binding!(FINDER, KeyCode::Enter, NONE, FinderAccept),
    binding!(FINDER, KeyCode::Esc, NONE, Back),
    binding!(FINDER, KeyCode::Char('c'), CONTROL, Back),
    // The watch list (§9). `w` is also the way out, so the overlay opens and
    // closes with one key the way the tree does with `t`.
    binding!(WATCH, KeyCode::Char('j'), NONE, WatchDown),
    binding!(WATCH, KeyCode::Char('k'), NONE, WatchUp),
    binding!(WATCH, KeyCode::Char('g'), NONE, WatchTop),
    binding!(WATCH, KeyCode::Char('G'), NONE, WatchBottom),
    binding!(WATCH, KeyCode::Enter, NONE, WatchAccept),
    binding!(WATCH, KeyCode::Char('w'), NONE, Back),
    binding!(WATCH, KeyCode::Esc, NONE, Back),
    binding!(WATCH, KeyCode::Char('q'), NONE, Back),
    binding!(WATCH, KeyCode::Char('c'), CONTROL, Quit),
    // The help overlay (§5).
    binding!(HELP, KeyCode::Char('j'), NONE, HelpDown),
    binding!(HELP, KeyCode::Char('k'), NONE, HelpUp),
    binding!(HELP, KeyCode::Char('f'), NONE, HelpPageDown),
    binding!(HELP, KeyCode::Char('b'), NONE, HelpPageUp),
    binding!(HELP, KeyCode::Char('g'), NONE, HelpTop),
    binding!(HELP, KeyCode::Char('G'), NONE, HelpBottom),
    binding!(HELP, KeyCode::Char('?'), NONE, Back),
    binding!(HELP, KeyCode::Esc, NONE, Back),
    binding!(HELP, KeyCode::Char('q'), NONE, Quit),
    binding!(HELP, KeyCode::Char('c'), CONTROL, Quit),
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
        // A control combination is spelled with the capital letter, the way
        // the terminal reports it; a bare letter is shown as it is typed so
        // that `n` and `N` do not become one row in the help.
        KeyCode::Char(letter) if binding.mods.contains(KeyModifiers::CONTROL) => {
            letter.to_uppercase().to_string()
        }
        KeyCode::Char(letter) => letter.to_string(),
        KeyCode::Enter => "Enter".to_owned(),
        KeyCode::Esc => "Esc".to_owned(),
        KeyCode::Tab => "Tab".to_owned(),
        other => format!("{other:?}"),
    };
    key.push_str(&name);
    key
}

/// One line of the help overlay: the key, and what it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpRow {
    pub key: String,
    pub does: &'static str,
}

/// A heading of the help overlay with the rows under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    pub rows: Vec<HelpRow>,
}

/// The whole help overlay, built from [`BINDINGS`] (§5).
///
/// A binding of both panes is shown once under `Everywhere` and the rest
/// under the context they belong to, so a key added to the table is a row
/// here without anything else being done.
pub fn help_sections() -> Vec<HelpSection> {
    let mut sections = vec![HelpSection {
        title: "Everywhere",
        rows: rows_in(BOTH),
    }];
    for (title, context) in [
        ("Tree", Context::Tree),
        ("Preview", Context::Preview),
        ("Finder", Context::Finder),
        ("Watch", Context::Watch),
        ("Help", Context::Help),
    ] {
        let rows: Vec<HelpRow> = BINDINGS
            .iter()
            .filter(|binding| binding.contexts != BOTH && binding.contexts.contains(&context))
            .map(row)
            .collect();
        if !rows.is_empty() {
            sections.push(HelpSection { title, rows });
        }
    }
    sections
}

/// The rows of the bindings that belong to exactly these contexts.
fn rows_in(contexts: &'static [Context]) -> Vec<HelpRow> {
    BINDINGS
        .iter()
        .filter(|binding| binding.contexts == contexts)
        .map(row)
        .collect()
}

fn row(binding: &Binding) -> HelpRow {
    HelpRow {
        key: describe(binding),
        does: binding.command.description(),
    }
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
    fn the_finder_holds_every_line_editing_key_the_specification_asks_for() {
        let expected: &[(KeyCode, KeyModifiers, Command)] = &[
            (KeyCode::Left, NONE, Command::FinderCursorLeft),
            (KeyCode::Right, NONE, Command::FinderCursorRight),
            (KeyCode::Char('b'), CONTROL, Command::FinderCursorLeft),
            (KeyCode::Char('f'), CONTROL, Command::FinderCursorRight),
            (KeyCode::Char('b'), ALT, Command::FinderCursorWordLeft),
            (KeyCode::Char('f'), ALT, Command::FinderCursorWordRight),
            (KeyCode::Home, NONE, Command::FinderCursorHome),
            (KeyCode::End, NONE, Command::FinderCursorEnd),
            (KeyCode::Char('a'), CONTROL, Command::FinderCursorHome),
            (KeyCode::Char('e'), CONTROL, Command::FinderCursorEnd),
            (KeyCode::Backspace, NONE, Command::FinderDeleteBack),
            (KeyCode::Char('h'), CONTROL, Command::FinderDeleteBack),
            (KeyCode::Delete, NONE, Command::FinderDeleteForward),
            (KeyCode::Char('d'), CONTROL, Command::FinderDeleteForward),
            (KeyCode::Char('w'), CONTROL, Command::FinderDeleteWord),
            (KeyCode::Char('u'), CONTROL, Command::FinderDeleteToStart),
            (KeyCode::Char('k'), CONTROL, Command::FinderDeleteToEnd),
            (KeyCode::Up, NONE, Command::FinderSelectUp),
            (KeyCode::Down, NONE, Command::FinderSelectDown),
            (KeyCode::Char('p'), CONTROL, Command::FinderSelectUp),
            (KeyCode::Char('n'), CONTROL, Command::FinderSelectDown),
            (KeyCode::PageUp, NONE, Command::FinderPageUp),
            (KeyCode::PageDown, NONE, Command::FinderPageDown),
            (KeyCode::Char('j'), ALT, Command::ScrollDown),
            (KeyCode::Char('k'), ALT, Command::ScrollUp),
            (KeyCode::Enter, NONE, Command::FinderAccept),
            (KeyCode::Esc, NONE, Command::Back),
            (KeyCode::Char('c'), CONTROL, Command::Back),
        ];
        for (code, mods, command) in expected {
            assert_eq!(
                command_for(Context::Finder, KeyEvent::new(*code, *mods)),
                Some(*command),
                "{code:?} {mods:?}"
            );
        }
    }

    #[test]
    fn ctrl_p_opens_the_list_from_the_panes_and_moves_the_row_inside_it() {
        for context in [Context::Tree, Context::Preview] {
            assert_eq!(
                command_for(context, ctrl(KeyCode::Char('p'))),
                Some(Command::OpenFiles),
                "{context:?}"
            );
        }
        // The specification keeps `Ctrl+P` as readline's "one row up" inside
        // the overlay: the selection moves, the overlay does not (§5, ※).
        assert_eq!(
            command_for(Context::Finder, ctrl(KeyCode::Char('p'))),
            Some(Command::FinderSelectUp)
        );
        assert_eq!(
            command_for(Context::Finder, ctrl(KeyCode::Char('n'))),
            Some(Command::FinderSelectDown)
        );
    }

    #[test]
    fn ctrl_g_opens_the_search_from_the_panes_and_is_the_way_between_the_lists() {
        for context in [Context::Tree, Context::Preview] {
            assert_eq!(
                command_for(context, ctrl(KeyCode::Char('g'))),
                Some(Command::OpenGrep),
                "{context:?}"
            );
        }
        // Inside an overlay it does not type and it does not select: it is
        // the switch between the file list and the search (§13.3).
        assert_eq!(
            command_for(Context::Finder, ctrl(KeyCode::Char('g'))),
            Some(Command::OpenGrep)
        );
    }

    #[test]
    fn keys_the_query_box_would_undo_or_break_a_line_with_are_unbound() {
        // The text box library binds these by default and would undo, redo or
        // break a line with them; yagura answers every key before the library
        // ever sees one, so none of them may resolve to a command (§13.1).
        for code in [
            KeyCode::Char('z'),
            KeyCode::Char('y'),
            KeyCode::Char('j'),
            KeyCode::Char('m'),
        ] {
            assert_eq!(
                command_for(Context::Finder, ctrl(code)),
                None,
                "{code:?} with control"
            );
        }
        // `Enter` is answered, but as the one command it means in here: take
        // the row. It never reaches the library, which would add a line.
        assert_eq!(
            command_for(Context::Finder, press(KeyCode::Enter)),
            Some(Command::FinderAccept)
        );
        // A plain letter belongs to the query box, so the finder table has
        // none of them at all.
        for code in [KeyCode::Char('j'), KeyCode::Char('k'), KeyCode::Char('h')] {
            assert_eq!(command_for(Context::Finder, press(code)), None, "{code:?}");
        }
    }

    #[test]
    fn a_key_added_to_the_table_is_a_row_of_the_help_overlay() {
        let listed: Vec<(String, &'static str)> = help_sections()
            .iter()
            .flat_map(|section| section.rows.iter().map(|row| (row.key.clone(), row.does)))
            .collect();
        for binding in BINDINGS {
            let wanted = row(binding);
            assert!(
                listed.contains(&(wanted.key, wanted.does)),
                "the help overlay has no row for {:?}",
                describe(binding)
            );
        }
    }

    #[test]
    fn bindings_describe_themselves_for_the_help_overlay() {
        let quit = bindings_for(Context::Tree)
            .find(|binding| binding.command == Command::Quit && binding.code == KeyCode::Char('q'))
            .expect("q binding");
        assert_eq!(describe(quit), "q");
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
