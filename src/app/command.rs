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
}
