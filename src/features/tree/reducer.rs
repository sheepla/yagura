//! The tree reducer: pure functions from state and messages to new state and
//! the work that has to happen next.

use std::path::PathBuf;

use crate::app::action::Action;
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::{AppState, Focus, Overlay, SortMode};
use crate::core::dirload::DirError;
use crate::core::fsmodel::{NodeId, NodeKind};
use crate::core::text;

use super::sort::{sort_all, sort_children};

/// Handles the messages that concern the tree.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    match action {
        Action::Command(command) => on_command(state, *command),
        Action::DirLoaded { .. } => on_dir_loaded(state, action),
        _ => Vec::new(),
    }
}

fn on_command(state: &mut AppState, command: Command) -> Vec<Effect> {
    let mut effects = Vec::new();
    match command {
        Command::Quit => state.quitting = true,
        Command::ToggleFocus => {
            state.focus = match state.focus {
                Focus::Tree => Focus::Preview,
                Focus::Preview => Focus::Tree,
            };
        }
        Command::Back => {
            // One step back: close an overlay, otherwise move the focus from
            // the preview to the tree.
            if state.overlay != Overlay::None {
                state.overlay = Overlay::None;
            } else if state.focus == Focus::Preview {
                state.focus = Focus::Tree;
            }
        }
        Command::ToggleTree => {
            state.tree_visible = !state.tree_visible;
            sync_viewport(state);
            crate::app::reduce::sync_preview_viewport(state);
        }

        Command::MoveDown => navigate(state, Navigation::Down),
        Command::MoveUp => navigate(state, Navigation::Up),
        Command::PageDown => navigate(state, Navigation::PageDown),
        Command::PageUp => navigate(state, Navigation::PageUp),
        Command::HalfPageDown => navigate(state, Navigation::HalfPageDown),
        Command::HalfPageUp => navigate(state, Navigation::HalfPageUp),
        Command::First => navigate(state, Navigation::First),
        Command::Last => navigate(state, Navigation::Last),

        Command::Expand => expand_selected(state, &mut effects),
        Command::Collapse => collapse_or_go_up(state),
        Command::ToggleExpand => toggle_selected(state, &mut effects),
        Command::OpenPreview => open_selected(state, &mut effects),

        // The preview pane owns the scroll commands and the reload, and the
        // root reducer owns the baseline toggle and the mode cycle; the tree
        // only has to not claim them, so that the root reducer can route
        // them there.
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
        | Command::CyclePreviewMode
        | Command::ToggleBaseline => {}
    }
    state.tree.refresh();
    effects
}

enum Navigation {
    Down,
    Up,
    PageDown,
    PageUp,
    HalfPageDown,
    HalfPageUp,
    First,
    Last,
}

fn navigate(state: &mut AppState, movement: Navigation) {
    // Navigating by hand leaves follow mode.
    state.follow = false;
    let tree = &mut state.tree;
    match movement {
        Navigation::Down => tree.move_selection(1),
        Navigation::Up => tree.move_selection(-1),
        Navigation::PageDown => tree.page(true),
        Navigation::PageUp => tree.page(false),
        Navigation::HalfPageDown => tree.half_page(true),
        Navigation::HalfPageUp => tree.half_page(false),
        Navigation::First => tree.select_first(),
        Navigation::Last => tree.select_last(),
    }
    follow_selection(state);
}

/// Points the preview at whatever the tree has selected.
fn follow_selection(state: &mut AppState) {
    let path = state.selected_path();
    if state.preview.path != path {
        state.preview.path = path;
        state.preview.scroll_line = 0;
        state.preview.scroll_col = 0;
    }
    // A new generation marks the pending load as the current one.
    state.preview.generation = state.preview.generation.wrapping_add(1);
}

fn expand_selected(state: &mut AppState, effects: &mut Vec<Effect>) {
    if let Some(node) = state.tree.selected {
        expand_node(state, node, effects);
    }
}

fn toggle_selected(state: &mut AppState, effects: &mut Vec<Effect>) {
    let Some(node) = state.tree.selected else {
        return;
    };
    if state.tree.is_expanded(node) {
        state.tree.collapse(node);
    } else {
        expand_node(state, node, effects);
    }
}

/// `Enter`: directories open, anything else hands over to the preview.
fn open_selected(state: &mut AppState, effects: &mut Vec<Effect>) {
    let Some(node) = state.tree.selected else {
        return;
    };
    if state
        .tree
        .model
        .get(node)
        .is_some_and(|entry| entry.kind == NodeKind::Dir)
    {
        toggle_selected(state, effects);
    } else {
        state.focus = Focus::Preview;
        follow_selection(state);
    }
}

/// `h`: close a directory, otherwise step out to the parent.
fn collapse_or_go_up(state: &mut AppState) {
    let Some(node) = state.tree.selected else {
        return;
    };
    if state.tree.is_expanded(node) {
        state.tree.collapse(node);
        state.tree.collapse_descendants(node);
        return;
    }
    if let Some(parent) = state.tree.model.get(node).and_then(|entry| entry.parent)
        && state.tree.model.contains(parent)
    {
        state.tree.select(parent);
        follow_selection(state);
    }
}

/// Expands a directory, asking for its listing the first time it is opened.
fn expand_node(state: &mut AppState, node: NodeId, effects: &mut Vec<Effect>) {
    state.tree.expand(node);
    request_children(state, node, effects);
}

/// Issues a directory load unless the listing is already in hand.
fn request_children(state: &mut AppState, node: NodeId, effects: &mut Vec<Effect>) {
    // Only a directory has children: asking for the listing of a file would
    // come back as an error and mark a perfectly readable file as unreadable.
    if !state
        .tree
        .model
        .get(node)
        .is_some_and(|entry| entry.is_expandable())
    {
        return;
    }
    let already_loaded = state
        .tree
        .model
        .get(node)
        .is_some_and(|entry| entry.loaded || entry.unreadable);
    if already_loaded || !state.tree.loading.insert(node) {
        return;
    }
    let generation = state.tree.load_gen.entry(node).or_insert(0);
    *generation = generation.wrapping_add(1);
    let generation = *generation;
    let Some(rel) = state.tree.model.rel_of(node).map(PathBuf::from) else {
        state.tree.loading.remove(&node);
        return;
    };
    effects.push(Effect::LoadDir {
        node,
        generation,
        rel,
    });
}

fn on_dir_loaded(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let Action::DirLoaded {
        node,
        generation,
        result,
    } = action
    else {
        return Vec::new();
    };
    state.tree.loading.remove(node);
    // The node may have been removed while the listing was in flight.
    if !state.tree.model.contains(*node) {
        return Vec::new();
    }
    // A newer request for the same node has been issued: this one is stale.
    if state.tree.load_gen.get(node).copied() != Some(*generation) {
        tracing::debug!(?node, generation, "dropping stale directory listing");
        return Vec::new();
    }
    match result {
        Ok(entries) => {
            state.tree.model.merge_children(*node, entries);
            sort_children(&mut state.tree.model, *node, state.tree.sort);
            // The listing pruned the rows of deleted files; the baseline
            // puts them back before the rows are shown again.
            crate::app::reduce::apply_changed(state);
        }
        Err(err) => {
            tracing::warn!(?node, %err, "directory listing failed");
            // The node stays in the tree, marked as unreadable.
            if let Some(entry) = state.tree.model.get_mut(*node) {
                entry.unreadable = true;
                entry.loaded = true;
            }
            state.set_error(format!("{}: {}", display_rel(state, *node), describe(err)));
            state.tree.bump();
        }
    }
    state.tree.refresh();
    Vec::new()
}

/// An error in the words of the user, not of the operating system.
fn describe(err: &DirError) -> String {
    match err {
        DirError::Io(err) => text::os_error(err),
    }
}

/// The path of a node as it should be shown to the user.
fn display_rel(state: &AppState, node: NodeId) -> String {
    state
        .tree
        .model
        .rel_of(node)
        .map(|rel| {
            if rel.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                crate::core::text::sanitize_os(rel.as_os_str())
            }
        })
        .unwrap_or_else(|| "?".to_owned())
}

/// Reapplies the sibling order after new children arrived.
pub fn sort_node(state: &mut AppState, node: NodeId) {
    sort_children(&mut state.tree.model, node, state.tree.sort);
}

/// Reapplies the sibling order to the whole tree.
pub fn resort_all(state: &mut AppState, mode: SortMode) {
    state.tree.sort = mode;
    sort_all(&mut state.tree.model, mode);
    state.tree.bump();
    state.tree.refresh();
}

/// Keeps the tree's row count in step with the terminal size.
pub fn sync_viewport(state: &mut AppState) {
    state.tree.set_viewport(state.tree_viewport_height());
}
