//! The tree reducer: pure functions from state and messages to new state and
//! the work that has to happen next.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::app::action::Action;
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::{AppState, Focus, Overlay, SortMode};
use crate::core::baseline::Phase;
use crate::core::dirload::DirError;
use crate::core::fsmodel::{NodeId, NodeKind};
use crate::core::fswatch::{ChangeKind, FsChange};
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
                let was_finder = state.overlay.is_finder();
                state.overlay = Overlay::None;
                // The pane the overlay drew over is the one showing again,
                // so it is the height a page of scrolling means now (§13.2).
                if was_finder {
                    crate::app::reduce::sync_preview_viewport(state);
                }
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

        Command::ToggleFollow => toggle_follow(state, &mut effects),
        Command::ToggleChangedOnly => toggle_changed_only(state),
        Command::CycleSort => cycle_sort(state),

        // The preview pane owns the scroll commands, the reload and the jumps
        // between hunks, and the root reducer owns the baseline toggle and the
        // mode cycle; the tree only has to not claim them, so that the root
        // reducer can route them there.
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
        | Command::NextHunk
        | Command::PrevHunk
        | Command::Reload
        | Command::CyclePreviewMode
        | Command::ToggleBaseline => {}
        // The finder overlays own their commands, and the root reducer routes
        // them there before they can reach this match (§13).
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
        | Command::FinderDeleteToEnd => {}
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

/// `F`: the selection follows the changes instead of the reader's hand.
///
/// Turning the mode on goes to the newest change straight away, so the key
/// does something the first time it is pressed (design §11.4).
fn toggle_follow(state: &mut AppState, effects: &mut Vec<Effect>) {
    state.follow = !state.follow;
    if state.follow {
        apply_follow(state, effects);
    }
}

/// Moves the selection onto the most recently changed file and lets the
/// preview follow it (design §11.4).
fn apply_follow(state: &mut AppState, effects: &mut Vec<Effect>) {
    if !state.follow {
        return;
    }
    let Some(rel) = newest_change(state) else {
        return;
    };
    let Some(target) = state.tree.model.lookup(&rel) else {
        return;
    };
    reveal(state, target, effects);
    state.tree.select(target);
    follow_selection(state);
}

/// The path that changed last, ties going to the one that sorts last so that
/// two runs of the same state pick the same row.
fn newest_change(state: &AppState) -> Option<PathBuf> {
    state
        .baseline
        .changed
        .iter()
        .max_by_key(|(_, entry)| entry.changed_at)
        .map(|(rel, _)| rel.clone())
}

/// Opens the way to a node: every directory above it, root first, so that
/// the row the caller is about to select is a row that exists.
fn reveal(state: &mut AppState, target: NodeId, effects: &mut Vec<Effect>) {
    let mut chain = Vec::new();
    let mut current = state.tree.model.get(target).and_then(|entry| entry.parent);
    while let Some(id) = current {
        chain.push(id);
        current = state.tree.model.get(id).and_then(|entry| entry.parent);
    }
    for id in chain.into_iter().rev() {
        expand_node(state, id, effects);
    }
}

/// Enter in the finder: the path becomes the selection and the preview (§4).
///
/// The file may sit behind listings the tree has not asked for yet, so the
/// jump is kept until every directory on the way has answered.
pub fn jump_to(state: &mut AppState, rel: PathBuf, effects: &mut Vec<Effect>) {
    // A row the filter hides is a row the reader cannot watch the jump land
    // on, so the filter comes off and says so (§4).
    if state.tree.changed_only {
        toggle_changed_only(state);
        state.set_info("the changed files filter was turned off");
    }
    state.tree.jump = Some(rel);
    finish_jump(state, effects);
}

/// Works towards the pending jump, asking for the listing that stands in the
/// way of the file; called again for every listing that arrives.
fn finish_jump(state: &mut AppState, effects: &mut Vec<Effect>) {
    let Some(rel) = state.tree.jump.clone() else {
        return;
    };
    // The deepest directory the model already knows on the way down to it.
    let mut prefix = PathBuf::new();
    let mut deepest = state.tree.model.root_id();
    for component in rel.components() {
        prefix.push(component);
        match state.tree.model.lookup(&prefix) {
            Some(node) => deepest = node,
            None => break,
        }
    }
    if let Some(target) = state.tree.model.lookup(&rel) {
        state.tree.jump = None;
        reveal(state, target, effects);
        state.tree.select(target);
        state.tree.refresh();
        if let Some(index) = state.tree.selected_index() {
            state.tree.scroll_into_view(index);
        }
        return;
    }
    // The listing that is in hand has no such path: the file is gone, and
    // waiting for a listing that will never arrive would keep the jump
    // pending forever.
    if state
        .tree
        .model
        .get(deepest)
        .is_some_and(|entry| entry.loaded)
    {
        state.tree.jump = None;
        return;
    }
    expand_node(state, deepest, effects);
}

/// `c`: only what changed, with the way to it left walkable (design §11.3).
fn toggle_changed_only(state: &mut AppState) {
    state.tree.changed_only = !state.tree.changed_only;
    state.tree.bump();
    state.tree.refresh();
    // The selection may have been on a row the filter took away; the row
    // closest to the top is as good a place to carry on from as any.
    if state.tree.selected_index().is_none() {
        state.tree.select_first();
    }
}

/// `o`: the other sibling order, applied to the whole tree at once.
fn cycle_sort(state: &mut AppState) {
    let mode = match state.tree.sort {
        SortMode::Name => SortMode::Recent,
        SortMode::Recent => SortMode::Name,
    };
    resort_all(state, mode);
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
    if already_loaded {
        return;
    }
    issue_load(state, node, effects);
}

/// Registers a listing request and asks for it (invariant I3).
///
/// A directory is watched before it is read, not after: a file created
/// between the two would be invisible until something else happened in the
/// directory (§7.1). One listing per directory is in flight at a time, so a
/// request that is already out changes nothing.
fn issue_load(state: &mut AppState, node: NodeId, effects: &mut Vec<Effect>) {
    if !state
        .tree
        .model
        .get(node)
        .is_some_and(|entry| entry.is_expandable())
    {
        return;
    }
    if !state.tree.loading.insert(node) {
        return;
    }
    let generation = state.tree.load_gen.entry(node).or_insert(0);
    *generation = generation.wrapping_add(1);
    let generation = *generation;
    let Some(rel) = state.tree.model.rel_of(node).map(PathBuf::from) else {
        state.tree.loading.remove(&node);
        return;
    };
    effects.push(Effect::WatchDirs {
        add: vec![rel.clone()],
        remove: Vec::new(),
    });
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
    // Where the selection sits now, in case this listing takes the row away.
    state.tree.refresh();
    let selected_row = state.tree.selected_index();
    // The first listing of a directory puts files in the tree that nothing
    // has watched until now: what they are is whatever the baseline said
    // when it was built, which may be from before they changed (§7.3).
    let first_listing = !state
        .tree
        .model
        .get(*node)
        .is_some_and(|entry| entry.loaded);
    let mut effects = Vec::new();
    match result {
        Ok(entries) => {
            let children = state.tree.model.merge_children(*node, entries);
            sort_children(&mut state.tree.model, *node, state.tree.sort);
            // The listing pruned the rows of deleted files; the baseline
            // puts them back before the rows are shown again.
            crate::app::reduce::apply_changed(state);
            if first_listing && state.now != SystemTime::UNIX_EPOCH {
                let at = state.now;
                let files = children
                    .into_iter()
                    .filter_map(|child| state.tree.model.get(child))
                    .filter(|child| !child.is_expandable())
                    .map(|child| child.rel.clone())
                    .collect();
                request_stats(state, at, files, &mut effects);
            }
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
    restore_selection(state, selected_row);
    state.tree.refresh();
    // A jump a finder asked for waits for the listings on its way in, and a
    // listing is the only thing that can bring it any closer (§4).
    if state.tree.jump.is_some() {
        finish_jump(state, &mut effects);
    }
    effects
}

/// Puts the selection back after a listing took the row it was on.
///
/// A file or a whole directory disappearing must not leave the cursor on a
/// node that no longer exists: the row at the same place is as close to "the
/// same spot" a list of siblings can offer (design §11.4).
fn restore_selection(state: &mut AppState, row: Option<usize>) {
    let Some(selected) = state.tree.selected else {
        return;
    };
    // The rows have to be rebuilt after the listing: reading them here would
    // read the ones the listing has just replaced.
    state.tree.refresh();
    if state.tree.model.contains(selected) {
        return;
    }
    let Some(row) = row else {
        return;
    };
    let index = row.min(state.tree.visible.len().saturating_sub(1));
    let Some(target) = state.tree.visible.get(index) else {
        return;
    };
    let target = target.node;
    state.tree.select(target);
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

/// Applies one batch of filesystem changes (design §7.3).
///
/// A batch is a hint, not a truth: nothing here reads the disk. What the
/// reducer already holds is stamped with the time of the change, and the
/// listings that would show a difference are asked for; the difference
/// itself arrives later as a `DirLoaded`, like any other listing.
pub fn on_fs_batch(state: &mut AppState, changes: &[FsChange], at: SystemTime) -> Vec<Effect> {
    let mut effects = Vec::new();
    let mut relists = BTreeSet::new();
    let mut stats = BTreeSet::new();
    let mut touched = BTreeSet::new();
    let mut head_may_have_moved = false;
    let mut ignore_changed = false;
    let mut preview_changed = false;

    for change in changes {
        let rel = change.rel.as_path();
        if rel.starts_with(".git") {
            // The repository metadata is not part of the tree: a new commit
            // reaches the baseline through a head check instead (§8.2).
            head_may_have_moved = true;
            continue;
        }
        if is_ignore_file(rel) {
            // Every listing in hand was made with the rules that just
            // changed, so every one of them has to be made again (§7.3).
            ignore_changed = true;
        }
        stamp(state, rel, at, &mut touched);
        // A change to the contents keeps the row it has; anything that adds
        // or takes a row away, or names a path the tree has never seen, has
        // to be looked up in the listing again.
        let unknown = state.tree.model.lookup(rel).is_none();
        if (change.kind != ChangeKind::Modify || unknown)
            && let Some(parent) = rel.parent()
        {
            relists.insert(parent.to_path_buf());
        }
        let listed = state.tree.model.lookup(rel);
        let is_directory = listed.is_some_and(|node| {
            state
                .tree
                .model
                .get(node)
                .is_some_and(|entry| entry.is_expandable())
        });
        if is_directory {
            // A directory has no status of its own, but every file below it
            // does - and once the directory is gone they are all changed at
            // once (§7.3).
            if change.kind == ChangeKind::Remove {
                stats.extend(descendant_files(state, rel));
            }
        } else {
            stats.insert(change.rel.clone());
        }
        if change.kind == ChangeKind::Remove && is_directory {
            // The directory is gone, and with it the watches below it; they
            // are dropped together so that the same path can be watched
            // again once something recreates it (§7.1).
            effects.push(Effect::WatchDirs {
                add: Vec::new(),
                remove: vec![change.rel.clone()],
            });
        }
        if state.preview.path.as_ref() == Some(&change.rel) {
            // The body on screen was read from a file that has just changed,
            // so it is read again without moving the view (§11.3).
            preview_changed = true;
        }
    }

    if ignore_changed {
        for node in loaded_dirs(state) {
            issue_load(state, node, &mut effects);
        }
    }
    for parent in relists {
        if let Some(node) = state.tree.model.lookup(&parent) {
            issue_load(state, node, &mut effects);
        }
    }
    if head_may_have_moved {
        effects.push(Effect::CheckHead);
    }
    request_stats(state, at, stats, &mut effects);
    if preview_changed {
        effects.extend(crate::features::preview::reducer::reload(state));
    }
    // The rows themselves only move when the order or the filter has
    // something to say about the times just written into them.
    if !touched.is_empty() && (state.tree.sort == SortMode::Recent || state.tree.changed_only) {
        if state.tree.sort == SortMode::Recent {
            for dir in &touched {
                sort_children(&mut state.tree.model, *dir, SortMode::Recent);
            }
        }
        state.tree.bump();
    }
    apply_follow(state, &mut effects);
    state.tree.refresh();
    effects
}

/// Reads every listing in hand again, after events were lost (§7.2, §7.4).
pub fn on_rescan(state: &mut AppState) -> Vec<Effect> {
    let mut effects = Vec::new();
    for node in loaded_dirs(state) {
        issue_load(state, node, &mut effects);
    }
    // The listings come back either way; what they cannot say is whether a
    // file they already held has changed, so every file in them is asked
    // about. Past the argument budget one `git status` answers for all of
    // them anyway (§7.4).
    let at = state.now;
    let files = loaded_files(state).into_iter().collect();
    request_stats(state, at, files, &mut effects);
    if state.preview.path.is_some() {
        effects.extend(crate::features::preview::reducer::reload(state));
    }
    effects
}

/// Registers a stats request for `paths` and asks for it (§7.3).
///
/// The generation is handed out per path, so that an answer to an older
/// request for one path cannot undo what a newer one has already said
/// (invariant I3). Without a baseline to compare against there is no answer
/// to give, and the question is not asked at all.
fn request_stats(
    state: &mut AppState,
    at: SystemTime,
    paths: BTreeSet<PathBuf>,
    effects: &mut Vec<Effect>,
) {
    // Before the first build has answered there is nothing to compare
    // against; either baseline can answer afterwards (§8.3).
    if state.baseline.phase != Phase::Ready {
        return;
    }
    let kind = state.baseline.kind;
    let requested: Vec<(PathBuf, u64)> = paths
        .into_iter()
        .filter(|rel| !rel.as_os_str().is_empty())
        .map(|rel| {
            let generation = state.stats_gen.entry(rel.clone()).or_insert(0);
            *generation += 1;
            (rel, *generation)
        })
        .collect();
    if !requested.is_empty() {
        effects.push(Effect::ComputeStats {
            kind,
            at,
            paths: requested,
        });
    }
}

/// The files below `rel`, for a directory that is being taken away.
fn descendant_files(state: &AppState, rel: &Path) -> Vec<PathBuf> {
    let Some(node) = state.tree.model.lookup(rel) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        let Some(entry) = state.tree.model.get(node) else {
            continue;
        };
        if entry.is_expandable() {
            stack.extend(state.tree.model.children(node));
        } else {
            found.push(entry.rel.clone());
        }
    }
    found
}

/// The files of every listing in hand: all of them could have changed while
/// nothing was listening (§7.4).
fn loaded_files(state: &AppState) -> Vec<PathBuf> {
    loaded_dirs(state)
        .into_iter()
        .flat_map(|dir| {
            state
                .tree
                .model
                .children(dir)
                .iter()
                .filter_map(move |child| {
                    let child = state.tree.model.get(*child)?;
                    (!child.is_expandable()).then(|| child.rel.clone())
                })
        })
        .collect()
}

/// The directories whose listing is in hand: only those can have gone stale.
fn loaded_dirs(state: &AppState) -> Vec<NodeId> {
    state
        .tree
        .model
        .iter()
        .filter(|(_, node)| node.is_expandable() && node.loaded)
        .map(|(node, _)| node)
        .collect()
}

/// True for the files that decide what a listing shows (§7.3).
fn is_ignore_file(rel: &Path) -> bool {
    matches!(
        rel.file_name().and_then(OsStr::to_str),
        Some(".gitignore" | ".ignore")
    )
}

/// Records when a path was last seen to change (spec, `LastChanged`).
///
/// The directories above it are stamped as well, so a collapsed row knows
/// how fresh it is without the view walking the subtree below it, and each
/// one is reported in `touched` so that the caller can reorder them.
fn stamp(state: &mut AppState, rel: &Path, at: SystemTime, touched: &mut BTreeSet<NodeId>) {
    if let Some(entry) = state.baseline.changed.get_mut(rel) {
        entry.changed_at = at;
    }
    let mut dir = rel;
    loop {
        if let Some(id) = state.tree.model.lookup(dir) {
            let mut is_dir = false;
            if let Some(node) = state.tree.model.get_mut(id) {
                node.changed_at = Some(at);
                is_dir = node.is_expandable();
            }
            if is_dir {
                touched.insert(id);
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::app::state::Config;
    use crate::cli::{BaselineKind, IconStyle};
    use crate::core::baseline::{ChangedEntry, Kind};
    use crate::core::fsmodel::{DirEntry, NodeKind, Status};

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

    fn root_entries() -> Vec<DirEntry> {
        vec![
            entry("README.md", NodeKind::File),
            entry("docs", NodeKind::Dir),
            entry("src", NodeKind::Dir),
        ]
    }

    /// A state whose root listing is in hand, with `docs` open as well.
    fn populated() -> AppState {
        let mut state = state();
        let root = state.tree.model.root_id();
        let generation = state.tree.load_gen[&root];
        crate::app::reduce::reduce(
            &mut state,
            Action::DirLoaded {
                node: root,
                generation,
                result: Ok(root_entries()),
            },
        );
        let docs = state.tree.model.lookup(Path::new("docs")).expect("docs");
        state.tree.select(docs);
        crate::app::reduce::reduce(&mut state, Action::Command(Command::Expand));
        let generation = state.tree.load_gen[&docs];
        crate::app::reduce::reduce(
            &mut state,
            Action::DirLoaded {
                node: docs,
                generation,
                result: Ok(vec![entry("guide.md", NodeKind::File)]),
            },
        );
        state
    }

    fn change(rel: &str, kind: ChangeKind) -> FsChange {
        FsChange {
            rel: PathBuf::from(rel),
            kind,
        }
    }

    fn load_dirs(effects: &[Effect]) -> Vec<NodeId> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::LoadDir { node, .. } => Some(*node),
                _ => None,
            })
            .collect()
    }

    fn at() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(60)
    }

    #[test]
    fn a_content_change_stamps_the_row_and_rereads_nothing() {
        let mut state = populated();
        let readme = state
            .tree
            .model
            .lookup(Path::new("README.md"))
            .expect("README");
        let effects = on_fs_batch(&mut state, &[change("README.md", ChangeKind::Modify)], at());
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(
            state.tree.model.get(readme).map(|node| node.changed_at),
            Some(Some(at()))
        );
        let root = state.tree.model.root_id();
        assert_eq!(
            state.tree.model.get(root).map(|node| node.changed_at),
            Some(Some(at())),
            "the directory above it is just as fresh"
        );
    }

    #[test]
    fn a_change_the_tree_has_never_seen_rereads_its_directory() {
        let mut state = populated();
        let root = state.tree.model.root_id();
        let effects = on_fs_batch(&mut state, &[change("new.txt", ChangeKind::Create)], at());
        assert_eq!(load_dirs(&effects), vec![root]);
        assert!(state.tree.loading.contains(&root));
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::WatchDirs { add, .. } if add == &vec![PathBuf::new()]
            )),
            "the directory is watched before it is read: {effects:?}"
        );
    }

    #[test]
    fn a_change_below_an_open_directory_rereads_that_directory() {
        let mut state = populated();
        let docs = state.tree.model.lookup(Path::new("docs")).expect("docs");
        let effects = on_fs_batch(
            &mut state,
            &[change("docs/page.md", ChangeKind::Create)],
            at(),
        );
        assert_eq!(load_dirs(&effects), vec![docs]);
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::WatchDirs { add, .. } if add == &vec![PathBuf::from("docs")]
            )),
            "the directory is watched before it is read: {effects:?}"
        );
    }

    #[test]
    fn the_shown_file_is_reread_when_it_changes() {
        let mut state = populated();
        state.preview.path = Some(PathBuf::from("README.md"));
        let effects = on_fs_batch(&mut state, &[change("README.md", ChangeKind::Modify)], at());
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::LoadPreview { rel, .. } if rel == &PathBuf::from("README.md")
            )),
            "the body on screen was not reread: {effects:?}"
        );
    }

    #[test]
    fn a_change_elsewhere_leaves_the_shown_file_alone() {
        let mut state = populated();
        state.preview.path = Some(PathBuf::from("README.md"));
        let effects = on_fs_batch(
            &mut state,
            &[change("docs/page.md", ChangeKind::Create)],
            at(),
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::LoadPreview { .. })),
            "a change elsewhere reloaded the preview: {effects:?}"
        );
    }

    #[test]
    fn an_ignore_file_rereads_every_listing_in_hand() {
        let mut state = populated();
        let root = state.tree.model.root_id();
        let docs = state.tree.model.lookup(Path::new("docs")).expect("docs");
        let effects = on_fs_batch(
            &mut state,
            &[change(".gitignore", ChangeKind::Modify)],
            at(),
        );
        let mut loaded = load_dirs(&effects);
        loaded.sort();
        let mut expected = vec![root, docs];
        expected.sort();
        assert_eq!(loaded, expected, "{effects:?}");
    }

    #[test]
    fn the_repository_metadata_reaches_nothing_in_the_tree() {
        let mut state = populated();
        let effects = on_fs_batch(&mut state, &[change(".git/HEAD", ChangeKind::Modify)], at());
        assert_eq!(effects, vec![Effect::CheckHead], "{effects:?}");
    }

    /// The paths a batch of changes asked to be checked, with the
    /// generation each was asked with.
    fn stats_requested(effects: &[Effect]) -> Vec<(PathBuf, u64)> {
        effects
            .iter()
            .flat_map(|effect| match effect {
                Effect::ComputeStats { paths, .. } => paths.clone(),
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn a_changed_file_is_asked_how_it_differs() {
        let mut state = populated();
        state.baseline.phase = Phase::Ready;
        let effects = on_fs_batch(
            &mut state,
            &[change("docs/guide.md", ChangeKind::Modify)],
            at(),
        );
        assert_eq!(
            effects,
            vec![Effect::ComputeStats {
                kind: Kind::Head,
                at: at(),
                paths: vec![(PathBuf::from("docs/guide.md"), 1)],
            }],
            "{effects:?}"
        );
        // The next question about the same path is a newer one, whose
        // answer cannot be mistaken for the first one's (invariant I3).
        let effects = on_fs_batch(
            &mut state,
            &[change("docs/guide.md", ChangeKind::Modify)],
            at(),
        );
        assert_eq!(
            effects,
            vec![Effect::ComputeStats {
                kind: Kind::Head,
                at: at(),
                paths: vec![(PathBuf::from("docs/guide.md"), 2)],
            }],
            "{effects:?}"
        );
    }

    #[test]
    fn only_a_finished_baseline_is_asked_about() {
        let mut state = populated();
        let effects = on_fs_batch(
            &mut state,
            &[change("docs/guide.md", ChangeKind::Modify)],
            at(),
        );
        assert!(
            effects.is_empty(),
            "before the first build has answered there is nothing to compare against: {effects:?}"
        );

        // A snapshot baseline answers the same question its own way (§8.3),
        // so the ask is made of either kind once a build has finished.
        state.baseline.kind = Kind::Snapshot;
        state.baseline.phase = Phase::Ready;
        let effects = on_fs_batch(
            &mut state,
            &[change("docs/guide.md", ChangeKind::Modify)],
            at(),
        );
        assert_eq!(
            effects,
            vec![Effect::ComputeStats {
                kind: Kind::Snapshot,
                at: at(),
                paths: vec![(PathBuf::from("docs/guide.md"), 1)],
            }],
            "{effects:?}"
        );
    }

    #[test]
    fn the_first_listing_asks_about_the_files_it_brings_in() {
        let mut state = state();
        state.baseline.phase = Phase::Ready;
        state.now = at();
        let root = state.tree.model.root_id();
        let generation = state.tree.load_gen[&root];
        let first = crate::app::reduce::reduce(
            &mut state,
            Action::DirLoaded {
                node: root,
                generation,
                result: Ok(root_entries()),
            },
        );
        assert_eq!(
            first,
            vec![Effect::ComputeStats {
                kind: Kind::Head,
                at: at(),
                paths: vec![(PathBuf::from("README.md"), 1)],
            }],
            "{first:?}"
        );
        // The same listing read again says nothing new about its files.
        let again = crate::app::reduce::reduce(
            &mut state,
            Action::DirLoaded {
                node: root,
                generation,
                result: Ok(root_entries()),
            },
        );
        assert!(again.is_empty(), "{again:?}");
    }

    #[test]
    fn a_removed_directory_asks_about_the_files_it_held() {
        let mut state = populated();
        state.baseline.phase = Phase::Ready;
        let root = state.tree.model.root_id();
        let effects = on_fs_batch(&mut state, &[change("docs", ChangeKind::Remove)], at());
        assert_eq!(load_dirs(&effects), vec![root]);
        assert_eq!(
            stats_requested(&effects),
            vec![(PathBuf::from("docs/guide.md"), 1)],
            "{effects:?}"
        );
    }

    #[test]
    fn a_rescan_asks_about_every_file_in_hand() {
        let mut state = populated();
        state.baseline.phase = Phase::Ready;
        state.now = at();
        let effects = on_rescan(&mut state);
        assert_eq!(
            stats_requested(&effects),
            vec![
                (PathBuf::from("README.md"), 1),
                (PathBuf::from("docs/guide.md"), 1),
            ],
            "{effects:?}"
        );
    }

    #[test]
    fn a_removed_directory_gives_up_its_watch() {
        let mut state = populated();
        let root = state.tree.model.root_id();
        let effects = on_fs_batch(&mut state, &[change("docs", ChangeKind::Remove)], at());
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::WatchDirs { remove, .. } if remove == &vec![PathBuf::from("docs")]
            )),
            "the watch of a directory that is gone was kept: {effects:?}"
        );
        assert_eq!(load_dirs(&effects), vec![root]);
    }

    #[test]
    fn a_rescan_reads_every_listing_in_hand_again() {
        let mut state = populated();
        let root = state.tree.model.root_id();
        let docs = state.tree.model.lookup(Path::new("docs")).expect("docs");
        state.preview.path = Some(PathBuf::from("README.md"));

        let effects = on_rescan(&mut state);
        assert_eq!(load_dirs(&effects), vec![root, docs]);
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::LoadPreview { .. })),
            "a file may have changed while the events were lost"
        );
    }

    #[test]
    fn a_selection_whose_row_is_gone_lands_on_the_row_in_its_place() {
        let mut state = populated();
        let docs = state.tree.model.lookup(Path::new("docs")).expect("docs");
        state.tree.select(docs);
        state.tree.refresh();
        assert_eq!(state.tree.selected_index(), Some(1));

        // The listing comes back without `docs`: the row it held is gone.
        let root = state.tree.model.root_id();
        let generation = state.tree.load_gen[&root];
        crate::app::reduce::reduce(
            &mut state,
            Action::DirLoaded {
                node: root,
                generation,
                result: Ok(vec![
                    entry("README.md", NodeKind::File),
                    entry("src", NodeKind::Dir),
                ]),
            },
        );
        let selected = state.tree.selected.expect("a selection");
        assert!(state.tree.model.contains(selected), "the selection is gone");
        assert_eq!(
            state
                .tree
                .model
                .get(selected)
                .map(|node| node.name.to_string_lossy().into_owned()),
            Some("src".to_owned())
        );
        assert_eq!(state.tree.selected_index(), Some(1), "the same row");
    }

    /// One path the baseline says differs, seen at `at`.
    fn differs(state: &mut AppState, rel: &str, at: SystemTime) {
        state.baseline.changed.insert(
            PathBuf::from(rel),
            ChangedEntry {
                status: Status::Modified,
                changed_at: at,
                stat: None,
            },
        );
    }

    /// The path of a node, as the tree holds it.
    fn rel_of(state: &AppState, node: NodeId) -> Option<PathBuf> {
        state.tree.model.get(node).map(|entry| entry.rel.clone())
    }

    /// The rows in view, indented the way the pane draws them.
    fn names(state: &AppState) -> Vec<String> {
        state
            .tree
            .visible
            .iter()
            .map(|row| {
                let node = state.tree.model.get(row.node).expect("node");
                format!(
                    "{}{}",
                    "  ".repeat(row.depth as usize),
                    node.name.to_string_lossy()
                )
            })
            .collect()
    }

    #[test]
    fn follow_takes_the_selection_to_the_newest_change_and_opens_the_way() {
        let mut state = populated();
        let docs = state.tree.model.lookup(Path::new("docs")).expect("docs");
        differs(&mut state, "README.md", at());
        differs(&mut state, "docs/guide.md", at() + Duration::from_secs(5));
        state.tree.collapse(docs);
        state.tree.refresh();

        crate::app::reduce::reduce(&mut state, Action::Command(Command::ToggleFollow));

        assert!(state.follow);
        let selected = state.tree.selected.expect("a selection");
        assert_eq!(
            rel_of(&state, selected),
            Some(PathBuf::from("docs/guide.md"))
        );
        assert!(
            state.tree.is_expanded(docs),
            "the way to the change was closed"
        );
        assert_eq!(
            state.preview.path,
            Some(PathBuf::from("docs/guide.md")),
            "the preview follows the selection"
        );
    }

    #[test]
    fn a_following_selection_moves_to_whatever_changed_last() {
        let mut state = populated();
        differs(&mut state, "README.md", at());
        crate::app::reduce::reduce(&mut state, Action::Command(Command::ToggleFollow));
        let first = state.tree.selected.expect("a selection");
        assert_eq!(rel_of(&state, first), Some(PathBuf::from("README.md")));

        let later = at() + Duration::from_secs(5);
        differs(&mut state, "docs/guide.md", later);
        on_fs_batch(
            &mut state,
            &[change("docs/guide.md", ChangeKind::Modify)],
            later,
        );

        let second = state.tree.selected.expect("a selection");
        assert_eq!(rel_of(&state, second), Some(PathBuf::from("docs/guide.md")));
    }

    #[test]
    fn c_shows_only_what_changed_and_carries_the_selection_out_of_the_way() {
        let mut state = populated();
        state.baseline.phase = Phase::Ready;
        let readme = state
            .tree
            .model
            .lookup(Path::new("README.md"))
            .expect("README");
        state.tree.select(readme);

        crate::app::reduce::reduce(&mut state, Action::Command(Command::ToggleChangedOnly));

        assert!(state.tree.changed_only);
        assert_eq!(names(&state), vec!["root"], "nothing differs yet");
        assert_eq!(
            state.tree.selected,
            Some(state.tree.model.root_id()),
            "the selection was on a row the filter took away"
        );

        differs(&mut state, "docs/guide.md", at());
        crate::app::reduce::apply_changed(&mut state);
        assert_eq!(names(&state), vec!["root", "  docs", "    guide.md"]);

        crate::app::reduce::reduce(&mut state, Action::Command(Command::ToggleChangedOnly));

        assert!(!state.tree.changed_only);
        assert_eq!(
            names(&state),
            vec!["root", "  docs", "    guide.md", "  src", "  README.md"],
            "the whole tree came back"
        );
    }

    #[test]
    fn o_switches_the_sibling_order_between_name_and_recency() {
        let mut state = populated();
        let docs = state.tree.model.lookup(Path::new("docs")).expect("docs");
        state.tree.model.merge_children(
            docs,
            &[
                entry("guide.md", NodeKind::File),
                entry("aaa.md", NodeKind::File),
            ],
        );
        sort_children(&mut state.tree.model, docs, SortMode::Name);
        state.tree.bump();
        state.tree.refresh();
        assert_eq!(names(&state)[2], "    aaa.md", "by name");

        let aaa = state
            .tree
            .model
            .lookup(Path::new("docs/aaa.md"))
            .expect("aaa");
        let guide = state
            .tree
            .model
            .lookup(Path::new("docs/guide.md"))
            .expect("guide");
        state.tree.model.get_mut(aaa).expect("aaa").changed_at = Some(at());
        state.tree.model.get_mut(guide).expect("guide").changed_at =
            Some(at() + Duration::from_secs(5));

        crate::app::reduce::reduce(&mut state, Action::Command(Command::CycleSort));

        assert_eq!(state.tree.sort, SortMode::Recent);
        assert_eq!(
            names(&state)[2],
            "    guide.md",
            "the newest change stands first"
        );
        assert_eq!(names(&state)[3], "    aaa.md");

        crate::app::reduce::reduce(&mut state, Action::Command(Command::CycleSort));

        assert_eq!(state.tree.sort, SortMode::Name);
        assert_eq!(names(&state)[2], "    aaa.md", "back to the name");
    }
}
