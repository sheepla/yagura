//! The vocabulary the keymap produces and the reducer consumes.
//!
//! State logic never inspects key codes, so a rebinding is a change to one
//! table and nothing else.

/// A user intent, independent of the key that triggered it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Quit,
    /// Move focus between the tree and the preview.
    ToggleFocus,
    /// One step back: close an overlay, then move focus from preview to tree.
    Back,
    ToggleTree,
    CyclePreviewMode,
    /// Switch between the git baseline and the startup snapshot.
    ToggleBaseline,

    // Tree navigation.
    MoveDown,
    MoveUp,
    PageDown,
    PageUp,
    HalfPageDown,
    HalfPageUp,
    First,
    Last,
    Expand,
    Collapse,
    ToggleExpand,
    /// Expand a directory, or move the focus to the preview.
    OpenPreview,

    // The way the tree is shown.
    /// Keep the selection on whatever changed last (design §11.4).
    ToggleFollow,
    /// Show only the paths that changed or were seen to change (§11.3).
    ToggleChangedOnly,
    /// The sibling order: by name, or by the most recent change.
    CycleSort,

    // Preview scrolling.
    ScrollDown,
    ScrollUp,
    ScrollPageDown,
    ScrollPageUp,
    ScrollHalfPageDown,
    ScrollHalfPageUp,
    ScrollTop,
    ScrollBottom,
    /// Sideways, because the preview does not wrap.
    ScrollLeft,
    ScrollRight,
    ScrollPageLeft,
    ScrollPageRight,
    /// The next / previous hunk of the diff, from either pane.
    NextHunk,
    PrevHunk,
    /// Read the shown file again, keeping the scroll where it is.
    ///
    /// A watch picks up a change on its own; this is for the case where it
    /// did not, and for a look at a file nothing has watched yet.
    Reload,

    // The finder overlay (§13).
    /// `Ctrl+P`: open the file finder.
    OpenFiles,
    /// `Ctrl+G`: open the grep list, or switch to it from the file finder
    /// (§13.3).
    OpenGrep,
    /// `Enter`: take the selected row and close the overlay (§4).
    FinderAccept,
    /// `↑` / `Ctrl+P`: the row above in the list.
    FinderSelectUp,
    /// `↓` / `Ctrl+N`: the row below in the list.
    FinderSelectDown,
    FinderPageUp,
    FinderPageDown,
    /// A character for the query box; the keymap never produces one for a
    /// plain key press, the reducer does (§13.1).
    FinderInsert(char),
    FinderCursorLeft,
    FinderCursorRight,
    FinderCursorWordLeft,
    FinderCursorWordRight,
    FinderCursorHome,
    FinderCursorEnd,
    FinderDeleteBack,
    FinderDeleteForward,
    FinderDeleteWord,
    FinderDeleteToStart,
    FinderDeleteToEnd,

    // The watch overlay (§9).
    /// `w`: the log of what has changed under the root.
    OpenWatch,
    WatchDown,
    WatchUp,
    WatchTop,
    /// `G`: back to the newest entry, which starts the following again.
    WatchBottom,
    /// `Enter`: go to the path of the selected entry (§9).
    WatchAccept,

    // The help overlay (§5).
    /// `?`: the keys, generated from the keymap itself.
    OpenHelp,
    HelpDown,
    HelpUp,
    HelpPageDown,
    HelpPageUp,
    HelpTop,
    HelpBottom,

    // The editor (§8).
    /// `e`: the shown file in `$VISUAL`, `$EDITOR` or `vi`.
    OpenEditor,
}

impl Command {
    /// What the help overlay says the command does (§5).
    ///
    /// Every variant is named here, so a command without a description does
    /// not compile and the overlay can never be missing a row.
    pub fn description(self) -> &'static str {
        match self {
            Self::Quit => "Quit",
            Self::ToggleFocus => "Move the focus between the tree and the preview",
            Self::Back => "Close the overlay, or step back from the preview to the tree",
            Self::ToggleTree => "Show or hide the tree",
            Self::CyclePreviewMode => "The next way to show the body",
            Self::ToggleBaseline => "Compare against HEAD or against the start of the session",

            Self::MoveDown => "The row below",
            Self::MoveUp => "The row above",
            Self::PageDown => "A page down",
            Self::PageUp => "A page up",
            Self::HalfPageDown => "Half a page down",
            Self::HalfPageUp => "Half a page up",
            Self::First => "The first row",
            Self::Last => "The last row",
            Self::Expand => "Expand the directory",
            Self::Collapse => "Collapse the directory, or go to the parent",
            Self::ToggleExpand => "Expand or collapse the directory",
            Self::OpenPreview => "Expand the directory, or focus the preview",

            Self::ToggleFollow => "Keep the selection on whatever changed last",
            Self::ToggleChangedOnly => "Show only the paths that changed",
            Self::CycleSort => "The sibling order: by name, or by the last change",

            Self::ScrollDown => "One line down",
            Self::ScrollUp => "One line up",
            Self::ScrollPageDown => "A page down",
            Self::ScrollPageUp => "A page up",
            Self::ScrollHalfPageDown => "Half a page down",
            Self::ScrollHalfPageUp => "Half a page up",
            Self::ScrollTop => "The first line",
            Self::ScrollBottom => "The last line",
            Self::ScrollLeft => "One column left",
            Self::ScrollRight => "One column right",
            Self::ScrollPageLeft => "A page of columns left",
            Self::ScrollPageRight => "A page of columns right",
            Self::NextHunk => "The next hunk of the diff",
            Self::PrevHunk => "The previous hunk of the diff",
            Self::Reload => "Read the shown file again",

            Self::OpenFiles => "Open the file finder",
            Self::OpenGrep => "Open the search through the files",
            Self::FinderAccept => "Take the row: the tree walks to it and the preview shows it",
            Self::FinderSelectUp => "The row above",
            Self::FinderSelectDown => "The row below",
            Self::FinderPageUp => "A page of rows up",
            Self::FinderPageDown => "A page of rows down",
            Self::FinderInsert(_) => "A character for the query box",
            Self::FinderCursorLeft => "One character left",
            Self::FinderCursorRight => "One character right",
            Self::FinderCursorWordLeft => "One word left",
            Self::FinderCursorWordRight => "One word right",
            Self::FinderCursorHome => "The start of the query",
            Self::FinderCursorEnd => "The end of the query",
            Self::FinderDeleteBack => "Delete the character before the cursor",
            Self::FinderDeleteForward => "Delete the character under the cursor",
            Self::FinderDeleteWord => "Delete the word before the cursor",
            Self::FinderDeleteToStart => "Delete to the start of the query",
            Self::FinderDeleteToEnd => "Delete to the end of the query",

            Self::OpenWatch => "Open the watch list",
            Self::WatchDown => "The entry below",
            Self::WatchUp => "The entry above",
            Self::WatchTop => "The oldest entry",
            Self::WatchBottom => "The newest entry, and follow it again",
            Self::WatchAccept => "Go to the path of the entry",

            Self::OpenHelp => "Show this list of keys",
            Self::HelpDown => "One line down",
            Self::HelpUp => "One line up",
            Self::HelpPageDown => "A page down",
            Self::HelpPageUp => "A page up",
            Self::HelpTop => "The first line",
            Self::HelpBottom => "The last line",

            Self::OpenEditor => "Open the shown file in $VISUAL or $EDITOR",
        }
    }
}
