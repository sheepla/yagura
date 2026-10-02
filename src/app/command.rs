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
    /// Watches are not in yet, so this is the only way to pick up a file that
    /// changed on disk.
    Reload,
}
