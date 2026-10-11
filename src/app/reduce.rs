//! The root reducer: the one place where actions meet the state.
//!
//! Every function here is pure: it reads the state, writes the state and
//! returns the work to do next. It never touches the filesystem, the clock or
//! a channel (invariants I1 and I2). Time arrives inside the actions.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::app::action::{Action, StatAnswer};
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::{AppState, Focus, Overlay};
use crate::core::baseline::{Built, ChangedEntry, Kind, Phase};
use crate::features::{finder, help, preview, tree, watch};
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
    if let Action::PreviewHighlights {
        rel,
        generation,
        spans,
        truncated,
    } = action
    {
        return preview::reducer::on_highlights(state, rel, generation, spans, truncated);
    }
    if let Action::BaselineReady { ver, built } = action {
        return apply_baseline(state, ver, built);
    }
    if let Action::StatsComputed { results } = action {
        return apply_stats(state, results);
    }
    if let Action::Command(Command::ToggleBaseline) = action {
        return toggle_baseline(state);
    }
    if let Action::Command(Command::CyclePreviewMode) = action {
        return cycle_preview_mode(state);
    }
    if let Action::Command(Command::OpenEditor) = action {
        return open_editor(state);
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
            // The matcher only advances while it is ticked, so the clock is
            // what a match still running waits for (§13.2). A rank that
            // settles on a different first row is a new preview target, so
            // the effects the overlay asks for are kept rather than dropped.
            if state.overlay.is_finder() {
                return finder_effects(state, &action);
            }
            Vec::new()
        }
        Action::MouseWheel { delta } => mouse_wheel(state, *delta),
        Action::MouseClick { column } => mouse_click(state, *column),
        Action::Key(_) | Action::Quit => {
            if matches!(action, Action::Quit) {
                state.quitting = true;
            }
            Vec::new()
        }
        // The two lists drawn over the panes take their own commands, and the
        // watch log needs to know where the reader has left the selection.
        Action::Command(command) if help::reducer::owns(*command) => {
            help::reducer::reduce(state, &action)
        }
        Action::Command(command) if watch::reducer::owns(*command) => watch_effects(state, &action),
        // The finder overlays take the keys that are theirs and every paste
        // aimed at their query box; the panes behind them keep the scrolling
        // and `Esc`, because one key means one thing (§4).
        Action::Command(command) if finder::reducer::owns(*command) => {
            finder_effects(state, &action)
        }
        Action::Paste(_) if state.overlay.is_finder() => finder_effects(state, &action),
        Action::FilesFound { .. } | Action::FilesScanDone { .. } => finder_effects(state, &action),
        // The answers of a grep run belong to the overlay that asked for
        // them (§13.3).
        Action::GrepChunk { .. }
        | Action::GrepDone { .. }
        | Action::GrepFailed { .. }
        | Action::GrepMissing { .. } => finder_effects(state, &action),
        Action::Paste(_)
        | Action::Command(_)
        | Action::DirLoaded { .. }
        | Action::PreviewLoaded { .. }
        | Action::PreviewHighlights { .. }
        | Action::BaselineReady { .. } => {
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
            // Closing an overlay with `Esc` lands here rather than with the
            // finder, so the pane gets the mode it is kept in back too.
            effects.extend(preview::reducer::reload_for_the_mode(state));
            effects
        }
        // Changes on disk belong to the tree, and the watch log keeps them as
        // well: it records whether or not its overlay is up (§12). The preview
        // is reloaded only for a path it is showing: what happens elsewhere in
        // the tree must not move the view (§7.3).
        Action::FsBatch { changes, at } => {
            watch::reducer::record(state, changes, *at);
            tree::reducer::on_fs_batch(state, changes, *at)
        }
        Action::FsRescanNeeded => tree::reducer::on_rescan(state),
        // A `HEAD` that moved is a new baseline to diff against, and only a
        // rebuild can say what differs below it (§8.2). Before the first
        // build has answered there is nothing to compare against, and a
        // rebuild of it would only be thrown away.
        Action::HeadChecked { oid } => {
            if state.baseline.kind != Kind::Head
                || state.baseline.phase != Phase::Ready
                || oid == &state.baseline.head_oid
            {
                return Vec::new();
            }
            state.baseline.ver += 1;
            vec![Effect::BuildBaseline {
                kind: state.baseline.kind,
                ver: state.baseline.ver,
            }]
        }
        // Handed to [`apply_stats`] above; here only so that every action
        // has a case.
        Action::StatsComputed { .. } => Vec::new(),
    }
}

/// An action for a finder overlay, followed by whatever the panes behind it
/// should know about the row the overlay has selected (§13.2).
fn finder_effects(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let mut effects = finder::reducer::reduce(state, action);
    effects.extend(preview::reducer::sync_target(state));
    // The overlay reads the file as content; when it lets go of the pane
    // (or takes it over), the mode the reader kept is read back (§13.2).
    effects.extend(preview::reducer::reload_for_the_mode(state));
    effects
}

/// An action for the watch list: leaving it means the panes show again, so
/// the preview is asked what it should be showing now (§9).
fn watch_effects(state: &mut AppState, action: &Action) -> Vec<Effect> {
    let mut effects = watch::reducer::reduce(state, action);
    effects.extend(preview::reducer::sync_target(state));
    effects
}

/// `e`: the file the reader is looking at, handed over to an editor outside
/// the terminal (§8).
///
/// The line the pane has scrolled to goes with it, and only from the pane:
/// the tree points at a row, not at a line inside one.
fn open_editor(state: &mut AppState) -> Vec<Effect> {
    let (rel, line) = match state.focus {
        Focus::Preview => match state.preview.path.clone() {
            Some(rel) => (rel, Some(state.preview.scroll_line + 1)),
            None => {
                state.set_info("there is no file to open");
                return Vec::new();
            }
        },
        Focus::Tree => {
            let Some(rel) = selected_file(state) else {
                state.set_info("there is no file to open");
                return Vec::new();
            };
            (rel, None)
        }
    };
    vec![Effect::RunEditor {
        path: state.root().join(rel),
        line,
    }]
}

/// The file under the cursor in the tree, and nothing when the cursor sits
/// on a directory.
fn selected_file(state: &AppState) -> Option<PathBuf> {
    let node = state.tree.selected?;
    let entry = state.tree.model.get(node)?;
    if entry.is_expandable() {
        return None;
    }
    Some(PathBuf::from(state.tree.model.rel_of(node)?))
}

/// The wheel turned: five lines of the pane of the moment, and one row a
/// time in the list an overlay draws (§4).
fn mouse_wheel(state: &mut AppState, delta: isize) -> Vec<Effect> {
    let down = delta >= 0;
    let command = match state.overlay {
        Overlay::None if state.focus == Focus::Preview => {
            state.preview.scroll_by(delta);
            return Vec::new();
        }
        Overlay::None => {
            if down {
                Command::MoveDown
            } else {
                Command::MoveUp
            }
        }
        Overlay::Help => {
            if down {
                Command::HelpDown
            } else {
                Command::HelpUp
            }
        }
        Overlay::Watch => {
            if down {
                Command::WatchDown
            } else {
                Command::WatchUp
            }
        }
        Overlay::Files | Overlay::Grep => {
            if down {
                Command::FinderSelectDown
            } else {
                Command::FinderSelectUp
            }
        }
    };
    // A list moves one row at a time; the whole notch is spent that way,
    // and every list clamps at its own ends.
    let mut effects = Vec::new();
    for _ in 0..delta.unsigned_abs() {
        effects.extend(reduce(state, Action::Command(command)));
    }
    effects
}

/// A click brings the pane it fell on the focus. An overlay keeps the
/// screen to itself for now: it is closed the way it was opened (§4).
fn mouse_click(state: &mut AppState, column: u16) -> Vec<Effect> {
    if !matches!(state.overlay, Overlay::None) {
        return Vec::new();
    }
    let area = Rect::new(0, 0, state.size.0, state.size.1);
    let areas = crate::ui::layout::compute(area, state.tree_visible);
    state.focus = if state.tree_visible && column < areas.tree.right() {
        Focus::Tree
    } else {
        Focus::Preview
    };
    Vec::new()
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
            | Command::NextHunk
            | Command::PrevHunk
            | Command::Reload
    )
}

/// `B`: switch between the git baseline and the startup snapshot.
///
/// Outside a repository there is nothing to toggle, so the key does nothing
/// (spec, key table).
fn toggle_baseline(state: &mut AppState) -> Vec<Effect> {
    if !state.baseline.repo {
        return Vec::new();
    }
    state.baseline.kind = state.baseline.kind.other();
    state.baseline.ver += 1;
    vec![Effect::BuildBaseline {
        kind: state.baseline.kind,
        ver: state.baseline.ver,
    }]
}

/// `s`: the next way to show the body.
///
/// Switching into or out of a diff re-reads the file, because the two kinds
/// of body are built differently; between the two diff modes the same rows
/// are simply drawn another way (§9.2).
fn cycle_preview_mode(state: &mut AppState) -> Vec<Effect> {
    let before = state.preview_mode.is_diff();
    state.preview_mode = state.preview_mode.next();
    if state.preview_mode == crate::app::state::PreviewMode::SideBySide
        && state.size.0 < crate::features::preview::view::SPLIT_MIN_COLS
    {
        // The view falls back to the inline diff on a terminal this narrow,
        // so the status bar says which one is actually on screen.
        state.set_info(format!(
            "side-by-side needs {} columns, using the inline diff",
            crate::features::preview::view::SPLIT_MIN_COLS
        ));
    }
    if before == state.preview_mode.is_diff() {
        // Between the two diff modes the same body is drawn another way, so
        // the read stays valid and only the record of it moves with the mode
        // (§9.2); leaving it behind would ask for the file to be read again
        // for nothing.
        state.preview.mode = state.preview_mode;
        return Vec::new();
    }
    preview::reducer::reload(state)
}

/// Swaps in a finished baseline. The previous one stays visible until this
/// moment, so a rebuild never blanks the tree (design, baseline section).
fn apply_baseline(state: &mut AppState, ver: u64, built: Built) -> Vec<Effect> {
    if ver != state.baseline.ver {
        return Vec::new();
    }
    state.baseline.kind = built.kind;
    state.baseline.phase = built.phase;
    state.baseline.repo = built.repo;
    state.baseline.head_oid = built.head_oid;
    state.baseline.changed = merge_changed(&state.baseline.changed, built.changed);
    let mut effects = Vec::new();
    if matches!(state.config.baseline, crate::cli::BaselineKind::Auto)
        && !built.repo
        && state.baseline.kind == Kind::Head
    {
        // `auto` degrades to a snapshot outside a repository and says so.
        // The rows keep what they show until that build answers, so the
        // switch never opens a gap (§8.4).
        state.baseline.kind = Kind::Snapshot;
        state.baseline.phase = Phase::Loading;
        state.baseline.ver += 1;
        effects.push(Effect::BuildBaseline {
            kind: Kind::Snapshot,
            ver: state.baseline.ver,
        });
    }
    if let Some(message) = built.message {
        state.set_info(message);
    }
    apply_changed(state);
    // A diff on screen was computed against the baseline that was just
    // replaced, so it is drawn again against the new one (invariant I3: the
    // reload's generation discards whatever is still in flight).
    if state.preview.path.is_some() && state.preview_mode.is_diff() {
        effects.extend(preview::reducer::reload(state));
    }
    effects
}

/// Writes one stats answer into the changed set (§7.3).
///
/// An answer to a request that has been superseded is dropped: the newer
/// one is already on its way, and the two would disagree only for as long
/// as it takes to arrive. A path that no longer differs at all leaves the
/// set, which is what takes its status off the row again.
fn apply_stats(state: &mut AppState, results: Vec<StatAnswer>) -> Vec<Effect> {
    let mut answered = false;
    for answer in results {
        if state.stats_gen.get(&answer.rel) != Some(&answer.generation) {
            continue;
        }
        answered = true;
        match answer.entry {
            None => {
                state.baseline.changed.remove(&answer.rel);
            }
            Some(mut entry) => {
                // What was first seen to change keeps that moment: the
                // answer says what is different now, not when it happened.
                if let Some(known) = state.baseline.changed.get(&answer.rel) {
                    entry.changed_at = known.changed_at;
                }
                state.baseline.changed.insert(answer.rel, entry);
            }
        }
    }
    if answered {
        apply_changed(state);
    }
    Vec::new()
}

/// Writes the changed set into the tree: every node gets its status, a file
/// deleted on disk keeps a row of its own, and the rows are rebuilt to show
/// them (design §11.1).
pub fn apply_changed(state: &mut AppState) {
    // No result has landed yet: there is nothing to mark, and the rows keep
    // the placeholders that say the baseline is still being counted. The
    // rows are rebuilt all the same, because this is often the call that
    // follows a new listing.
    if state.baseline.phase != crate::core::baseline::Phase::Loading {
        state.tree.model.sync_baseline(&state.baseline.changed);
    }
    state.tree.bump();
    state.tree.refresh();
}

/// The fresh status replaces the set, but a path the app has already seen
/// keeps the time it was first recorded at: only a watch can observe a newer
/// change, and it writes that time into the entry itself (design, git
/// baseline section).
fn merge_changed(
    old: &BTreeMap<PathBuf, ChangedEntry>,
    new: BTreeMap<PathBuf, ChangedEntry>,
) -> BTreeMap<PathBuf, ChangedEntry> {
    new.into_iter()
        .map(|(path, mut entry)| {
            if let Some(previous) = old.get(&path) {
                entry.changed_at = previous.changed_at;
            }
            (path, entry)
        })
        .collect()
}

/// Gives the preview pane its height from the current layout.
pub fn sync_preview_viewport(state: &mut AppState) {
    state.preview.viewport = state.preview_viewport_height();
    state.preview.clamp();
}

/// Turns raw input into a command using the keymap for the current context.
/// `None` means the key is not bound and the action is dropped.
///
/// A finder overlay is the one place where a key press is not looked up: a
/// character the key meant literally belongs to the query box, whatever the
/// table says (§13.1). The named keys - `Enter`, `Esc`, every control key -
/// are still the keymap's to decide.
fn resolve(state: &AppState, action: Action) -> Option<Action> {
    match action {
        Action::Key(key) => {
            if state.overlay.is_finder()
                && let Some(c) = plain_char(key)
            {
                return Some(Action::Command(Command::FinderInsert(c)));
            }
            keymap::command_for(state.context(), key).map(Action::Command)
        }
        other => Some(other),
    }
}

/// The character a key press meant, when it meant that and nothing else.
fn plain_char(key: KeyEvent) -> Option<char> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    // An uppercase letter arrives as itself plus `SHIFT`, which is a literal
    // character and not a modifier of it.
    key.modifiers
        .difference(KeyModifiers::SHIFT)
        .is_empty()
        .then_some(c)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::*;
    use crate::app::command::Command;
    use crate::app::state::{Config, Focus, Overlay, PreviewMode};
    use crate::cli::{BaselineKind, IconStyle};
    use crate::input::task::WHEEL_LINES;

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
            "focus={:?} parked={:?} mode={:?} overlay={:?} tree_visible={} follow={} quit={} sel={:?} scroll={} rows={:?} msg={:?}",
            state.focus,
            state.focus_before_hide,
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
        reduce(&mut state, key(KeyCode::Char('t')));
        assert!(state.tree_visible, "`t` did not show the tree again");
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
    fn the_editor_is_asked_for_the_file_the_tree_has_selected() {
        use std::ffi::OsStr;

        use crate::core::fsmodel::NodeKind;

        let mut state = state();
        let root = state.tree.model.root_id();
        let file = state
            .tree
            .model
            .insert_child(root, OsStr::new("a.txt"), NodeKind::File);
        state.tree.selected = Some(file);

        let effects = reduce(&mut state, Action::Command(Command::OpenEditor));
        let [Effect::RunEditor { path, line }] = effects.as_slice() else {
            panic!("the editor was not asked for: {effects:?}");
        };
        assert_eq!(path, &PathBuf::from("/root").join("a.txt"));
        assert_eq!(*line, None, "the tree points at a row, not at a line");
    }

    #[test]
    fn the_editor_is_told_the_line_the_pane_is_showing() {
        let mut state = state();
        state.focus = Focus::Preview;
        state.preview.path = Some(PathBuf::from("a.rs"));
        state.preview.scroll_line = 41;

        let effects = reduce(&mut state, Action::Command(Command::OpenEditor));
        let [Effect::RunEditor { path, line }] = effects.as_slice() else {
            panic!("the editor was not asked for: {effects:?}");
        };
        assert_eq!(path, &PathBuf::from("/root").join("a.rs"));
        assert_eq!(*line, Some(42), "the first line on screen is one based");
    }

    #[test]
    fn a_directory_under_the_cursor_is_nothing_to_open() {
        let mut state = state();
        state.focus = Focus::Tree;

        let effects = reduce(&mut state, Action::Command(Command::OpenEditor));
        assert!(effects.is_empty(), "{effects:?}");
        assert!(
            state.visible_message().is_some(),
            "the status bar says why nothing happened"
        );
    }

    /// A key press carrying modifiers, which the plain `key` helper never does.
    fn key_with(code: KeyCode, mods: KeyModifiers) -> Action {
        Action::Key(KeyEvent::new(code, mods))
    }

    #[test]
    fn an_open_finder_takes_the_plain_keys_and_leaves_the_text_box_library_out() {
        let mut state = state();
        reduce(
            &mut state,
            key_with(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(state.overlay, Overlay::Files);
        // A plain letter would scroll a pane or move a row; in here it types.
        reduce(&mut state, key(KeyCode::Char('j')));
        assert_eq!(state.finder.query_text(), "j");
        assert_eq!(
            state.focus,
            Focus::Tree,
            "the panes behind the overlay keep their focus"
        );
        // What the query box library would undo, redo or break a line with is
        // bound nowhere, so it arrives as no command at all (§13.1).
        reduce(
            &mut state,
            key_with(KeyCode::Char('z'), KeyModifiers::CONTROL),
        );
        reduce(
            &mut state,
            key_with(KeyCode::Char('y'), KeyModifiers::CONTROL),
        );
        assert_eq!(state.finder.query_text(), "j");
        // `Ctrl+P` in here moves the selection rather than opening anything.
        reduce(
            &mut state,
            key_with(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(state.overlay, Overlay::Files);
        // And `Esc` closes the overlay in one step (§4).
        reduce(&mut state, key(KeyCode::Esc));
        assert_eq!(state.overlay, Overlay::None);
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

    /// A row the selection cannot leave is not a new target: the pane keeps
    /// its body and the line it was read up to.
    #[test]
    fn a_navigation_that_cannot_move_leaves_the_pane_where_it_was() {
        let mut state = tree_with_rows();
        reduce(&mut state, Action::Command(Command::Last));
        state.preview.set_doc(scrollable());
        state.preview.scroll_line = 7;

        let effects = reduce(&mut state, Action::Command(Command::MoveDown));
        assert_eq!(state.preview.scroll_line, 7, "the view does not jump");
        assert!(state.preview.doc.is_some(), "the body stayed");
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::LoadPreview { .. })),
            "a selection that cannot move was read again: {effects:?}"
        );
    }

    /// A document long enough for the wheel to have somewhere to go.
    fn scrollable() -> crate::core::preview::Doc {
        crate::core::preview::Doc {
            rel: PathBuf::from("a.txt"),
            name: "a.txt".into(),
            size: 1,
            language: crate::core::preview::Language::Text,
            lines: (0..50).map(|index| format!("line {index}")).collect(),
            truncated: false,
            too_large: false,
            spans: Vec::new(),
            highlight_truncated: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: None,
            diff_note: None,
            formatted: false,
        }
    }

    #[test]
    fn the_wheel_moves_the_focused_pane_five_lines_a_notch() {
        let mut state = state();
        state.focus = Focus::Preview;
        state.preview.viewport = 10;
        state.preview.set_doc(scrollable());

        reduce(&mut state, Action::MouseWheel { delta: WHEEL_LINES });
        assert_eq!(state.preview.scroll_line, 5, "five lines a notch");
        reduce(
            &mut state,
            Action::MouseWheel {
                delta: -WHEEL_LINES,
            },
        );
        assert_eq!(state.preview.scroll_line, 0, "and five back");
    }

    /// The tree with rows under the root, so the selection has somewhere to
    /// go.
    fn tree_with_rows() -> AppState {
        use crate::core::fsmodel::{DirEntry, NodeKind};
        let mut state = state();
        let root = state.tree.model.root_id();
        let rows: Vec<DirEntry> = (0..10)
            .map(|index| DirEntry {
                name: format!("f{index}.txt").into(),
                kind: NodeKind::File,
                link_target: None,
                unreadable: false,
            })
            .collect();
        state.tree.model.merge_children(root, &rows);
        state.tree.bump();
        state.tree.expand(root);
        state.tree.refresh();
        state.tree.selected = state.tree.model.lookup(std::path::Path::new("f0.txt"));
        state.focus = Focus::Tree;
        state
    }

    #[test]
    fn the_wheel_walks_the_selection_when_the_tree_has_the_focus() {
        let mut state = tree_with_rows();

        reduce(&mut state, Action::MouseWheel { delta: WHEEL_LINES });
        let selected = state
            .tree
            .selected
            .and_then(|node| state.tree.model.get(node));
        assert_eq!(
            selected.map(|entry| entry.name.to_string_lossy().into_owned()),
            Some("f5.txt".to_owned()),
            "five rows down"
        );

        reduce(
            &mut state,
            Action::MouseWheel {
                delta: -WHEEL_LINES,
            },
        );
        let selected = state
            .tree
            .selected
            .and_then(|node| state.tree.model.get(node));
        assert_eq!(
            selected.map(|entry| entry.name.to_string_lossy().into_owned()),
            Some("f0.txt".to_owned()),
            "five rows back"
        );
    }

    #[test]
    fn the_wheel_scrolls_the_list_an_overlay_is_showing() {
        let mut state = state();
        state.overlay = Overlay::Help;

        reduce(&mut state, Action::MouseWheel { delta: WHEEL_LINES });
        assert!(state.help.scroll > 0, "the help list moved");
    }

    #[test]
    fn a_click_brings_the_pane_it_fell_on_the_focus() {
        let mut state = state();
        state.focus = Focus::Preview;
        reduce(&mut state, Action::MouseClick { column: 0 });
        assert_eq!(state.focus, Focus::Tree, "the tree lives at the left");

        let tree_width = crate::ui::layout::tree_width(state.size.0);
        reduce(&mut state, Action::MouseClick { column: tree_width });
        assert_eq!(
            state.focus,
            Focus::Preview,
            "the preview begins where the tree ends"
        );
    }

    #[test]
    fn a_click_with_the_tree_hidden_finds_no_tree() {
        let mut state = state();
        state.tree_visible = false;
        state.focus = Focus::Tree;

        reduce(&mut state, Action::MouseClick { column: 0 });
        assert_eq!(
            state.focus,
            Focus::Preview,
            "the whole width belongs to the preview"
        );
    }

    #[test]
    fn a_click_while_an_overlay_is_up_is_ignored() {
        let mut state = state();
        state.overlay = Overlay::Help;
        state.focus = Focus::Tree;

        reduce(&mut state, Action::MouseClick { column: 60 });
        assert_eq!(
            state.focus,
            Focus::Tree,
            "the overlay keeps the screen to itself"
        );
    }

    #[test]
    fn with_the_tree_hidden_the_keys_scroll_the_reader() {
        let mut state = tree_with_rows();
        // The pane is showing the file the tree has selected, so hiding the
        // tree does not ask for another one.
        state.preview.path = Some(PathBuf::from("f0.txt"));
        state.preview.set_doc(scrollable());

        reduce(&mut state, key(KeyCode::Char('t')));
        assert!(!state.tree_visible);
        assert_eq!(
            state.focus,
            Focus::Preview,
            "a pane nobody can see cannot hold the keys"
        );

        reduce(&mut state, key(KeyCode::Char('j')));
        assert_eq!(state.preview.scroll_line, 1, "`j` scrolled the reader");
        reduce(&mut state, key(KeyCode::Char('k')));
        assert_eq!(state.preview.scroll_line, 0, "`k` scrolled it back");
        let selected = state
            .tree
            .selected
            .and_then(|node| state.tree.model.get(node));
        assert_eq!(
            selected.map(|entry| entry.name.to_string_lossy().into_owned()),
            Some("f0.txt".to_owned()),
            "the keys left the tree alone"
        );
    }

    #[test]
    fn showing_the_tree_takes_the_focus_back_where_it_was() {
        let mut state = tree_with_rows();

        reduce(&mut state, key(KeyCode::Char('t')));
        assert_eq!(state.focus, Focus::Preview, "the reader got the keys");
        reduce(&mut state, key(KeyCode::Char('t')));
        assert!(state.tree_visible);
        assert_eq!(state.focus, Focus::Tree, "the tree got its keys back");

        state.focus = Focus::Preview;
        reduce(&mut state, key(KeyCode::Char('t')));
        reduce(&mut state, key(KeyCode::Char('t')));
        assert_eq!(
            state.focus,
            Focus::Preview,
            "showing the tree does not steal the keys"
        );
    }

    #[test]
    fn the_keys_do_not_focus_a_hidden_tree() {
        let mut state = tree_with_rows();
        reduce(&mut state, key(KeyCode::Char('t')));
        assert_eq!(state.focus, Focus::Preview);

        reduce(&mut state, key(KeyCode::Tab));
        assert_eq!(
            state.focus,
            Focus::Preview,
            "there is one pane on the screen to focus"
        );
        reduce(&mut state, key(KeyCode::Esc));
        assert_eq!(
            state.focus,
            Focus::Preview,
            "`Esc` does not step into a hidden tree"
        );
    }

    fn entry(status: crate::core::fsmodel::Status, at: SystemTime) -> ChangedEntry {
        ChangedEntry {
            status,
            changed_at: at,
            stat: None,
        }
    }

    fn built(
        repo: bool,
        head_oid: Option<&str>,
        changed: BTreeMap<PathBuf, ChangedEntry>,
    ) -> Built {
        Built {
            kind: Kind::Head,
            phase: if repo {
                crate::core::baseline::Phase::Ready
            } else {
                crate::core::baseline::Phase::Unavailable
            },
            repo,
            head_oid: head_oid.map(str::to_owned),
            changed,
            message: None,
        }
    }

    #[test]
    fn a_stale_baseline_result_is_dropped() {
        use crate::core::fsmodel::Status;

        let mut state = state();
        state.baseline.repo = true;
        let first = built(
            true,
            Some("aaa"),
            BTreeMap::from([(
                PathBuf::from("a.txt"),
                entry(Status::Modified, SystemTime::UNIX_EPOCH),
            )]),
        );
        reduce(
            &mut state,
            Action::BaselineReady {
                ver: 1,
                built: first,
            },
        );
        assert_eq!(state.baseline.head_oid.as_deref(), Some("aaa"));

        // Ticking the build number over for the other baseline.
        let effects = reduce(&mut state, Action::Command(Command::ToggleBaseline));
        assert_eq!(
            effects,
            vec![Effect::BuildBaseline {
                kind: Kind::Snapshot,
                ver: 2
            }]
        );
        assert_eq!(state.baseline.kind, Kind::Snapshot);

        let stale = built(
            true,
            Some("bbb"),
            BTreeMap::from([(
                PathBuf::from("b.txt"),
                entry(Status::Modified, SystemTime::UNIX_EPOCH),
            )]),
        );
        reduce(
            &mut state,
            Action::BaselineReady {
                ver: 1,
                built: stale,
            },
        );
        assert_eq!(
            state.baseline.head_oid.as_deref(),
            Some("aaa"),
            "a result from an older build replaced the current one"
        );
        assert!(
            state
                .baseline
                .changed
                .contains_key(PathBuf::from("a.txt").as_path())
        );
    }

    #[test]
    fn b_swaps_the_baseline_without_a_pause_between_the_two() {
        use crate::core::fsmodel::Status;

        let mut state = state();
        state.baseline.repo = true;
        let head = built(
            true,
            Some("aaa"),
            BTreeMap::from([(
                PathBuf::from("a.txt"),
                entry(Status::Modified, SystemTime::UNIX_EPOCH),
            )]),
        );
        reduce(
            &mut state,
            Action::BaselineReady {
                ver: 1,
                built: head,
            },
        );

        let effects = reduce(&mut state, Action::Command(Command::ToggleBaseline));
        assert_eq!(
            effects,
            vec![Effect::BuildBaseline {
                kind: Kind::Snapshot,
                ver: 2,
            }]
        );
        assert_eq!(state.baseline.kind, Kind::Snapshot);
        assert_eq!(
            state.baseline.phase,
            Phase::Ready,
            "the swap waits for the answer rather than blanking the rows"
        );
        assert!(
            state
                .baseline
                .changed
                .contains_key(PathBuf::from("a.txt").as_path()),
            "the old marks are still on screen until the new baseline lands"
        );

        let snapshot = Built {
            kind: Kind::Snapshot,
            phase: Phase::Ready,
            repo: true,
            head_oid: None,
            changed: BTreeMap::new(),
            message: None,
        };
        let effects = reduce(
            &mut state,
            Action::BaselineReady {
                ver: 2,
                built: snapshot,
            },
        );
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(state.baseline.kind, Kind::Snapshot);
        assert_eq!(state.baseline.phase, Phase::Ready);
        assert!(
            state.baseline.changed.is_empty(),
            "the rows now say what the snapshot says"
        );
    }

    #[test]
    fn auto_falls_back_to_a_snapshot_outside_a_repository() {
        let mut state = state();
        let built = Built {
            kind: Kind::Head,
            phase: crate::core::baseline::Phase::Unavailable,
            repo: false,
            head_oid: None,
            changed: BTreeMap::new(),
            message: Some("not a git repository".to_owned()),
        };
        let effects = reduce(&mut state, Action::BaselineReady { ver: 1, built });
        assert_eq!(state.baseline.kind, Kind::Snapshot);
        assert_eq!(
            state.baseline.phase,
            crate::core::baseline::Phase::Loading,
            "the snapshot that replaces it is being built"
        );
        assert_eq!(
            effects,
            vec![Effect::BuildBaseline {
                kind: Kind::Snapshot,
                ver: 2,
            }],
            "{effects:?}"
        );
        assert_eq!(
            state.visible_message().map(|message| message.text.as_str()),
            Some("not a git repository")
        );
    }

    #[test]
    fn b_does_nothing_outside_a_repository() {
        let mut state = state();
        assert!(!state.baseline.repo);
        let effects = reduce(&mut state, Action::Command(Command::ToggleBaseline));
        assert!(effects.is_empty());
        assert_eq!(state.baseline.kind, Kind::Head);
    }

    #[test]
    fn a_rebuild_keeps_the_first_change_time_of_a_path() {
        use crate::core::fsmodel::Status;

        let mut state = state();
        let first_seen = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let first = built(
            true,
            None,
            BTreeMap::from([(PathBuf::from("a.txt"), entry(Status::Modified, first_seen))]),
        );
        reduce(
            &mut state,
            Action::BaselineReady {
                ver: 1,
                built: first,
            },
        );

        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(50);
        let rebuild = built(
            true,
            None,
            BTreeMap::from([
                (PathBuf::from("a.txt"), entry(Status::Deleted, later)),
                (PathBuf::from("c.txt"), entry(Status::Added, later)),
            ]),
        );
        state.baseline.ver = 2;
        reduce(
            &mut state,
            Action::BaselineReady {
                ver: 2,
                built: rebuild,
            },
        );
        assert_eq!(
            state.baseline.changed[PathBuf::from("a.txt").as_path()].changed_at,
            first_seen
        );
        assert_eq!(
            state.baseline.changed[PathBuf::from("c.txt").as_path()].changed_at,
            later
        );
        assert_eq!(state.baseline.changed.len(), 2);
    }

    #[test]
    fn cycling_into_or_out_of_a_diff_reloads_the_body() {
        let mut state = state();
        state.preview.path = Some(PathBuf::from("a.rs"));

        // Content to InlineDiff: the body has to be rebuilt as a diff.
        let effects = reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        assert_eq!(state.preview_mode, PreviewMode::InlineDiff);
        let generation = state.preview.generation;
        match effects.into_iter().next().expect("a reload") {
            Effect::LoadPreview {
                mode,
                baseline_ver,
                generation: loaded,
                ..
            } => {
                assert_eq!(mode, PreviewMode::InlineDiff);
                assert_eq!(baseline_ver, state.baseline.ver);
                assert_eq!(loaded, generation);
            }
            other => panic!("expected a reload, got {other:?}"),
        }

        // InlineDiff to SideBySide: the same rows, drawn another way.
        assert!(
            reduce(&mut state, Action::Command(Command::CyclePreviewMode)).is_empty(),
            "the rows do not change between the diff modes"
        );
        assert_eq!(state.preview_mode, PreviewMode::SideBySide);

        // SideBySide to Content: the plain body again.
        let effects = reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        assert_eq!(state.preview_mode, PreviewMode::Content);
        match effects.into_iter().next().expect("a reload") {
            Effect::LoadPreview { mode, .. } => assert_eq!(mode, PreviewMode::Content),
            other => panic!("expected a reload, got {other:?}"),
        }
    }

    #[test]
    fn switching_out_of_a_diff_waits_instead_of_drawing_its_rows_twice() {
        use crate::core::diff::Diff;
        use crate::features::preview::PreviewStatus;

        let mut state = state();
        state.preview_mode = PreviewMode::SideBySide;
        state.preview.path = Some(PathBuf::from("a.rs"));
        // The diff is on the pane: it was read for the diff mode, so its
        // `lines` are folded rows rather than the file's own lines.
        state.preview.mode = PreviewMode::SideBySide;
        let mut doc = scrollable();
        doc.diff = Some(Diff::compute("old", "new").expect("a diff"));
        state.preview.set_doc(doc);
        state.preview.scroll_line = 7;

        let effects = reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        assert_eq!(state.preview_mode, PreviewMode::Content);
        assert!(
            state.preview.doc.is_none(),
            "the folded rows must not be drawn as plain lines"
        );
        assert_eq!(state.preview.preview, PreviewStatus::Loading);
        assert_eq!(state.preview.path, Some(PathBuf::from("a.rs")));
        assert_eq!(state.preview.scroll_line, 7, "the view does not jump");
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::LoadPreview {
                    mode: PreviewMode::Content,
                    ..
                }
            )),
            "the plain body is on its way: {effects:?}"
        );
    }

    #[test]
    fn the_finder_reads_content_and_the_reader_gets_the_diff_back() {
        let mut state = state();
        state.preview_mode = PreviewMode::InlineDiff;

        reduce(&mut state, Action::Command(Command::OpenFiles));
        let generation = state.finder.scan_gen;
        let effects = reduce(
            &mut state,
            Action::FilesFound {
                generation,
                rels: vec![PathBuf::from("a.rs")],
            },
        );
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::LoadPreview {
                    mode: PreviewMode::Content,
                    ..
                }
            )),
            "the overlay reads the file as content (design §13.2): {effects:?}"
        );

        let effects = reduce(&mut state, Action::Command(Command::FinderAccept));
        assert_eq!(state.overlay, Overlay::None);
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::LoadPreview {
                    mode: PreviewMode::InlineDiff,
                    ..
                }
            )),
            "the pane gets the mode the reader kept back (spec §7): {effects:?}"
        );
        assert_eq!(state.preview.mode, PreviewMode::InlineDiff);
    }

    #[test]
    fn escaping_the_finder_gives_the_diff_back_too() {
        use std::ffi::OsStr;

        use crate::core::fsmodel::NodeKind;

        let mut state = state();
        state.preview_mode = PreviewMode::InlineDiff;
        let root = state.tree.model.root_id();
        let file = state
            .tree
            .model
            .insert_child(root, OsStr::new("a.rs"), NodeKind::File);
        state.tree.selected = Some(file);
        // The overlay has been reading the file the tree points at.
        state.overlay = Overlay::Files;
        state.preview.path = Some(PathBuf::from("a.rs"));
        state.preview.mode = PreviewMode::Content;

        let effects = reduce(&mut state, Action::Command(Command::Back));
        assert_eq!(state.overlay, Overlay::None);
        assert!(
            effects.iter().any(|effect| matches!(
                effect,
                Effect::LoadPreview {
                    mode: PreviewMode::InlineDiff,
                    ..
                }
            )),
            "Esc lands with the pane: {effects:?}"
        );
    }

    #[test]
    fn a_new_baseline_redraws_a_diff_but_leaves_the_content_alone() {
        let mut state = state();
        state.preview.path = Some(PathBuf::from("a.rs"));
        state.preview_mode = PreviewMode::InlineDiff;

        let effects = reduce(
            &mut state,
            Action::BaselineReady {
                ver: 1,
                built: built(true, None, BTreeMap::new()),
            },
        );
        match effects.into_iter().next().expect("a reload") {
            Effect::LoadPreview {
                mode, baseline_ver, ..
            } => {
                assert_eq!(mode, PreviewMode::InlineDiff);
                assert_eq!(baseline_ver, 1);
            }
            other => panic!("expected a reload, got {other:?}"),
        }

        state.preview_mode = PreviewMode::Content;
        state.baseline.ver = 2;
        assert!(
            reduce(
                &mut state,
                Action::BaselineReady {
                    ver: 2,
                    built: built(true, None, BTreeMap::new()),
                },
            )
            .is_empty(),
            "the plain body does not depend on the baseline"
        );
    }

    #[test]
    fn splitting_on_a_narrow_terminal_says_which_diff_is_shown() {
        let mut state = state();
        assert!(state.size.0 < crate::features::preview::view::SPLIT_MIN_COLS);
        state.preview.path = Some(PathBuf::from("a.rs"));

        reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        assert!(
            state.visible_message().is_none(),
            "the inline diff fits any width"
        );

        reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        assert_eq!(state.preview_mode, PreviewMode::SideBySide);
        let message = state.visible_message().expect("a message");
        assert!(message.text.contains("100 columns"), "{:?}", message.text);
        assert!(message.text.contains("inline"), "{:?}", message.text);
    }

    #[test]
    fn splitting_on_a_wide_terminal_says_nothing() {
        let mut state = state();
        state.size = (120, 40);
        state.preview.path = Some(PathBuf::from("a.rs"));

        reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        reduce(&mut state, Action::Command(Command::CyclePreviewMode));
        assert_eq!(state.preview_mode, PreviewMode::SideBySide);
        assert!(state.visible_message().is_none());
    }

    #[test]
    fn the_baseline_marks_the_tree_and_a_listing_keeps_the_deleted_rows() {
        use std::ffi::OsStr;
        use std::path::Path;

        use crate::core::fsmodel::{DirEntry, NodeKind, Status};

        let mut state = state();
        let root = state.tree.model.root_id();
        let docs = state
            .tree
            .model
            .insert_child(root, OsStr::new("docs"), NodeKind::Dir);
        let readme = state
            .tree
            .model
            .insert_child(root, OsStr::new("README.md"), NodeKind::File);
        state.tree.load_gen.insert(docs, 1);

        reduce(
            &mut state,
            Action::BaselineReady {
                ver: 1,
                built: built(
                    true,
                    Some("aaa"),
                    BTreeMap::from([
                        (
                            PathBuf::from("README.md"),
                            entry(Status::Deleted, SystemTime::UNIX_EPOCH),
                        ),
                        (
                            PathBuf::from("docs/gone.txt"),
                            entry(Status::Deleted, SystemTime::UNIX_EPOCH),
                        ),
                    ]),
                ),
            },
        );
        assert_eq!(
            state.tree.model.get(readme).map(|node| node.status),
            Some(Status::Deleted)
        );
        let gone = state
            .tree
            .model
            .lookup(Path::new("docs/gone.txt"))
            .expect("the deleted file keeps a row");
        assert_eq!(
            state.tree.model.get(gone).map(|node| node.kind),
            Some(NodeKind::File)
        );

        // The listing of docs has no gone.txt in it, so the merge prunes the
        // row; the baseline puts it back before the frame is drawn.
        reduce(
            &mut state,
            Action::DirLoaded {
                node: docs,
                generation: 1,
                result: Ok(vec![DirEntry {
                    name: OsStr::new("other.txt").to_os_string(),
                    kind: NodeKind::File,
                    link_target: None,
                    unreadable: false,
                }]),
            },
        );
        assert!(
            state
                .tree
                .model
                .lookup(Path::new("docs/gone.txt"))
                .is_some(),
            "the row of the deleted file vanished after a listing"
        );
        assert_eq!(
            state.tree.model.get(readme).map(|node| node.status),
            Some(Status::Deleted),
            "the listing of another directory cleared a status"
        );
    }

    /// A baseline that is in hand, with `a.txt` known to differ from it.
    fn with_a_changed_file() -> AppState {
        use crate::core::fsmodel::Status;

        let mut state = state();
        state.baseline.repo = true;
        reduce(
            &mut state,
            Action::BaselineReady {
                ver: 1,
                built: built(
                    true,
                    Some("aaa"),
                    BTreeMap::from([(
                        PathBuf::from("a.txt"),
                        entry(Status::Modified, SystemTime::UNIX_EPOCH),
                    )]),
                ),
            },
        );
        state
    }

    #[test]
    fn an_answer_to_a_stats_request_that_was_superseded_is_dropped() {
        use crate::core::fsmodel::Status;

        let mut state = with_a_changed_file();
        state.stats_gen.insert(PathBuf::from("a.txt"), 2);
        let effects = reduce(
            &mut state,
            Action::StatsComputed {
                results: vec![StatAnswer {
                    rel: PathBuf::from("a.txt"),
                    generation: 1,
                    entry: Some(entry(
                        Status::Added,
                        SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                    )),
                }],
            },
        );
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(
            state.baseline.changed[Path::new("a.txt")].status,
            Status::Modified,
            "an answer from a request that had been superseded landed"
        );
    }

    #[test]
    fn an_answer_that_says_a_file_no_longer_differs_takes_it_out_of_the_set() {
        let mut state = with_a_changed_file();
        state.stats_gen.insert(PathBuf::from("a.txt"), 1);
        reduce(
            &mut state,
            Action::StatsComputed {
                results: vec![StatAnswer {
                    rel: PathBuf::from("a.txt"),
                    generation: 1,
                    entry: None,
                }],
            },
        );
        assert!(
            !state.baseline.changed.contains_key(Path::new("a.txt")),
            "the file is still marked as changed: {:?}",
            state.baseline.changed
        );
    }

    #[test]
    fn an_answer_keeps_the_moment_the_change_was_first_seen() {
        use crate::core::fsmodel::Status;

        let first_seen = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let mut state = state();
        state.baseline.repo = true;
        reduce(
            &mut state,
            Action::BaselineReady {
                ver: 1,
                built: built(
                    true,
                    Some("aaa"),
                    BTreeMap::from([(PathBuf::from("a.txt"), entry(Status::Modified, first_seen))]),
                ),
            },
        );
        state.stats_gen.insert(PathBuf::from("a.txt"), 1);
        reduce(
            &mut state,
            Action::StatsComputed {
                results: vec![StatAnswer {
                    rel: PathBuf::from("a.txt"),
                    generation: 1,
                    entry: Some(entry(
                        Status::Added,
                        SystemTime::UNIX_EPOCH + Duration::from_secs(50),
                    )),
                }],
            },
        );
        assert_eq!(
            state.baseline.changed[Path::new("a.txt")].status,
            Status::Added
        );
        assert_eq!(
            state.baseline.changed[Path::new("a.txt")].changed_at,
            first_seen
        );
    }

    #[test]
    fn a_head_check_rebuilds_only_when_the_head_really_moved() {
        let mut state = with_a_changed_file();
        assert_eq!(state.baseline.ver, 1);

        let effects = reduce(
            &mut state,
            Action::HeadChecked {
                oid: Some("aaa".to_owned()),
            },
        );
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(state.baseline.ver, 1);

        let effects = reduce(
            &mut state,
            Action::HeadChecked {
                oid: Some("bbb".to_owned()),
            },
        );
        assert_eq!(
            effects,
            vec![Effect::BuildBaseline {
                kind: Kind::Head,
                ver: 2
            }],
            "{effects:?}"
        );
        assert_eq!(state.baseline.ver, 2);

        // A snapshot baseline has nothing to do with where `HEAD` points,
        // so a move under it costs no rebuild at all.
        let effects = reduce(&mut state, Action::Command(Command::ToggleBaseline));
        assert_eq!(
            effects,
            vec![Effect::BuildBaseline {
                kind: Kind::Snapshot,
                ver: 3
            }]
        );
        let effects = reduce(
            &mut state,
            Action::HeadChecked {
                oid: Some("ccc".to_owned()),
            },
        );
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(state.baseline.ver, 3);
    }

    #[test]
    fn a_head_check_has_nothing_to_compare_against_before_the_first_build() {
        let mut state = state();
        assert_eq!(state.baseline.phase, Phase::Loading);
        let effects = reduce(
            &mut state,
            Action::HeadChecked {
                oid: Some("aaa".to_owned()),
            },
        );
        assert!(effects.is_empty(), "{effects:?}");
        assert_eq!(state.baseline.ver, 1);
    }
}
