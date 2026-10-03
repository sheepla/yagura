//! The preview reducer: what the preview pane does with a selection and a
//! result.
//!
//! Like every other reducer it is pure. Reading the file happens in
//! [`crate::core::preview`] on a worker and comes back as an action.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app::action::Action;
use crate::app::command::Command;
use crate::app::effect::Effect;
use crate::app::state::{AppState, Focus, Overlay, PreviewMode};
use crate::core::preview::{Doc, PreviewError};
use crate::core::text;
use crate::features::preview::state::PreviewStatus;

/// Applies a command that belongs to the preview.
pub fn reduce(state: &mut AppState, action: &Action) -> Vec<Effect> {
    match action {
        Action::Command(command) => on_command(state, *command),
        _ => Vec::new(),
    }
}

fn on_command(state: &mut AppState, command: Command) -> Vec<Effect> {
    // A reload is about what is on screen, so it works from either pane.
    if matches!(command, Command::Reload) {
        return reload(state);
    }
    // So does a jump between hunks, but only a drawn diff has hunks.
    if matches!(command, Command::NextHunk | Command::PrevHunk) {
        return jump_hunk(state, command);
    }
    // Scrolling is the preview's own business and only when it has focus or
    // when an overlay is borrowing the pane; the same key means something
    // else in the tree.
    if state.focus != Focus::Preview && !state.overlay.is_finder() {
        return Vec::new();
    }
    let page = state.preview.height() as isize;
    let width = body_width(state) as isize;
    match command {
        Command::ScrollDown => state.preview.scroll_by(1),
        Command::ScrollUp => state.preview.scroll_by(-1),
        Command::ScrollPageDown => state.preview.scroll_by(page),
        Command::ScrollPageUp => state.preview.scroll_by(-page),
        Command::ScrollHalfPageDown => state.preview.scroll_by(page / 2),
        Command::ScrollHalfPageUp => state.preview.scroll_by(-(page / 2)),
        Command::ScrollTop => state.preview.scroll_line = 0,
        Command::ScrollBottom => state.preview.scroll_line = state.preview.max_scroll(),
        Command::ScrollLeft => state.preview.scroll_by_columns(-1, width as usize),
        Command::ScrollRight => state.preview.scroll_by_columns(1, width as usize),
        Command::ScrollPageLeft => state.preview.scroll_by_columns(-width, width as usize),
        Command::ScrollPageRight => state.preview.scroll_by_columns(width, width as usize),
        _ => return Vec::new(),
    }
    Vec::new()
}

/// `n` / `N`: the next or the previous hunk of the diff.
///
/// The rows of the document are the rows of the diff, so a hunk's index is
/// a line to scroll to. Outside a diff, or with no such jump left in the
/// direction asked for, the view stays where it is.
fn jump_hunk(state: &mut AppState, command: Command) -> Vec<Effect> {
    if !state.preview_mode.is_diff() {
        return Vec::new();
    }
    let target = state
        .preview
        .doc
        .as_ref()
        .and_then(|doc| doc.diff.as_ref())
        .and_then(|diff| {
            let here = state.preview.scroll_line;
            match command {
                Command::NextHunk => diff.hunks.iter().copied().find(|hunk| *hunk > here),
                Command::PrevHunk => diff.hunks.iter().rev().copied().find(|hunk| *hunk < here),
                _ => None,
            }
        });
    if let Some(target) = target {
        state.preview.scroll_line = target;
        state.preview.clamp();
    }
    Vec::new()
}

/// The columns of text a sideways scroll can show.
///
/// The gutter is part of the pane but stays put, so it comes back out of the
/// count before a scroll is measured.
pub fn body_width(state: &AppState) -> usize {
    let area = if state.overlay.is_finder() {
        crate::ui::layout::finder(state.area()).preview
    } else {
        crate::ui::layout::compute(state.area(), state.tree_visible).preview
    };
    let text = area.width.saturating_sub(PANE_MARGIN) as usize;
    text.saturating_sub(super::view::GUTTER_COLS)
}

/// Columns of margin the preview pane keeps on each side.
const PANE_MARGIN: u16 = 1;

/// Takes the document that just arrived, if it belongs to the current target.
pub fn on_loaded(
    state: &mut AppState,
    rel: PathBuf,
    generation: u64,
    result: Result<Doc, PreviewError>,
) -> Vec<Effect> {
    if !state.preview.is_current(&rel, generation) {
        // A newer selection won the race (invariant I3).
        tracing::debug!(?rel, generation, "dropping stale preview");
        return Vec::new();
    }
    match result {
        Ok(doc) => {
            state.preview.set_doc(doc);
            Vec::new()
        }
        Err(err) => {
            state.preview.set_failed();
            let reason = match &err {
                PreviewError::Io(err) => text::os_error(err),
            };
            state.set_error(format!(
                "cannot preview {}: {reason}",
                text::sanitize_os(rel.as_os_str())
            ));
            Vec::new()
        }
    }
}

/// Points the preview at whatever should be showing, and returns the load to
/// run, or nothing when the target did not change, which is the common case:
/// every tree event comes through here.
///
/// While a finder is open the row it has selected is what the reader is
/// looking at, and a jump that has not landed yet is where the preview is
/// headed; the tree has its turn again once neither is true (§13.2).
pub fn sync_target(state: &mut AppState) -> Vec<Effect> {
    if state.overlay.is_finder() || state.tree.jump.is_some() {
        // While the grep list is up the pane is aimed at the line of the row
        // it has selected, so the reader lands where the row points (§13.3).
        if state.overlay == Overlay::Grep {
            let hit = state.finder.selected_match();
            state.preview.aim_at(hit);
        }
        let overlay = state.overlay;
        let rel = state
            .tree
            .jump
            .clone()
            .or_else(|| state.finder.selected_path(overlay));
        return match rel {
            Some(rel) => aim(state, rel),
            None => {
                if state.preview.preview != PreviewStatus::Empty {
                    state.preview.clear();
                }
                Vec::new()
            }
        };
    }
    let Some(node) = state.tree.selected else {
        if state.preview.preview != PreviewStatus::Empty {
            state.preview.clear();
        }
        return Vec::new();
    };
    let Some(entry) = state.tree.model.get(node) else {
        return Vec::new();
    };
    if entry.is_expandable() {
        if state.preview.preview != PreviewStatus::Empty {
            state.preview.clear();
        }
        return Vec::new();
    }
    let Some(rel) = state.tree.model.rel_of(node).map(Path::to_path_buf) else {
        return Vec::new();
    };
    aim(state, rel)
}

/// Points the preview at `rel` unless it is already there.
fn aim(state: &mut AppState, rel: PathBuf) -> Vec<Effect> {
    state.preview.aimed_at(&rel);
    if state.preview.path.as_ref() == Some(&rel) {
        return Vec::new();
    }
    let generation = state.preview.target(rel.clone());
    vec![load(state, rel, generation)]
}

/// Re-reads the shown file without moving the view.
///
/// Used by the reload path; the generation still advances, so a result from
/// before the request is dropped.
pub fn reload(state: &mut AppState) -> Vec<Effect> {
    let Some(rel) = state.preview.path.clone() else {
        return Vec::new();
    };
    let generation = state.preview.generation + 1;
    state.preview.generation = generation;
    vec![load(state, rel, generation)]
}

/// Reads the shown file again when the finder and the pane have traded who
/// reads it: the overlay reads the file as content whatever the reader has
/// chosen (design §13.2), and the pane draws the mode the reader kept
/// (spec §7). The two bodies are built differently, so the mode the pane
/// needs is the mode that has to be read.
pub fn reload_for_the_mode(state: &mut AppState) -> Vec<Effect> {
    if state.preview.path.is_none() || state.preview.mode == mode_for(state) {
        return Vec::new();
    }
    reload(state)
}

/// The mode a load is asked for: content while the finder is over the pane,
/// the reader's own mode otherwise.
fn mode_for(state: &AppState) -> PreviewMode {
    if state.overlay.is_finder() {
        PreviewMode::Content
    } else {
        state.preview_mode
    }
}

/// The load the pane needs for what it is currently showing: the mode says
/// whether a diff is drawn, and the build number pins which baseline it is
/// drawn against (§8, §9.2).
fn load(state: &mut AppState, rel: PathBuf, generation: u64) -> Effect {
    let mode = mode_for(state);
    // What was asked for is remembered, so the pane knows when the file has
    // to be read again in another mode (§13.2).
    state.preview.mode = mode;
    Effect::LoadPreview {
        rel,
        generation,
        mode,
        baseline_ver: state.baseline.ver,
    }
}

/// The document of the shown file, if one has arrived.
pub fn doc(state: &AppState) -> Option<&Arc<Doc>> {
    state.preview.doc.as_ref()
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::app::state::{AppState, Config};
    use crate::cli::{BaselineKind, IconStyle};
    use crate::core::fsmodel::{FsModel, NodeKind};
    use crate::core::preview::{Doc, Language, PreviewError};

    fn state() -> AppState {
        let mut state = AppState::new(
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
        );
        state.preview.viewport = 10;
        state
    }

    /// A state with one file, selected.
    fn with_file() -> AppState {
        let mut state = state();
        let mut model = FsModel::new(PathBuf::from("/root"));
        let root = model.root_id();
        let file = model.insert_child(root, std::ffi::OsStr::new("a.rs"), NodeKind::File);
        state.tree.model = model;
        state.tree.select(file);
        state
    }

    fn sample(rel: &str, lines: &[&str]) -> Doc {
        sample_owned(rel, lines.iter().map(|line| (*line).to_owned()).collect())
    }

    fn sample_owned(rel: &str, lines: Vec<String>) -> Doc {
        Doc {
            rel: PathBuf::from(rel),
            name: rel.into(),
            size: 1,
            language: Language::Rust,
            lines,
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

    fn load_effects(effects: Vec<Effect>) -> Vec<(PathBuf, u64)> {
        effects
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::LoadPreview {
                    rel, generation, ..
                } => Some((rel, generation)),
                _ => None,
            })
            .collect()
    }

    /// Delivers a result the way the effect runner would.
    fn deliver(
        state: &mut AppState,
        rel: &str,
        generation: u64,
        result: Result<Doc, PreviewError>,
    ) -> Vec<Effect> {
        on_loaded(state, PathBuf::from(rel), generation, result)
    }

    #[test]
    fn selecting_a_file_asks_for_it_exactly_once() {
        let mut state = with_file();
        let first = load_effects(sync_target(&mut state));
        assert_eq!(first, vec![(PathBuf::from("a.rs"), 1)]);
        assert_eq!(state.preview.preview, PreviewStatus::Loading);
        // The same selection again changes nothing.
        assert!(sync_target(&mut state).is_empty());
    }

    #[test]
    fn selecting_a_directory_clears_the_preview() {
        let mut state = with_file();
        sync_target(&mut state);
        let root = state.tree.model.root_id();
        state.tree.select(root);
        assert!(sync_target(&mut state).is_empty());
        assert_eq!(state.preview.preview, PreviewStatus::Empty);
        assert!(state.preview.path.is_none());
    }

    #[test]
    fn a_result_for_the_current_generation_is_accepted() {
        let mut state = with_file();
        let mut effects = sync_target(&mut state);
        let Effect::LoadPreview {
            rel, generation, ..
        } = effects.pop().expect("load")
        else {
            panic!("expected a preview load");
        };
        let rel = rel.to_string_lossy().to_string();
        assert!(
            deliver(
                &mut state,
                &rel,
                generation,
                Ok(sample("a.rs", &["fn main() {}"]))
            )
            .is_empty()
        );
        assert_eq!(state.preview.preview, PreviewStatus::Ready);
        assert_eq!(state.preview.total_lines, 1);
    }

    #[test]
    fn a_result_from_a_superseded_request_is_dropped() {
        let mut state = with_file();
        sync_target(&mut state);
        deliver(&mut state, "a.rs", 0, Ok(sample("a.rs", &["stale"])));
        assert_eq!(state.preview.preview, PreviewStatus::Loading);
        assert!(doc(&state).is_none(), "a stale document reached the pane");
    }

    #[test]
    fn a_result_for_another_file_is_dropped() {
        let mut state = with_file();
        sync_target(&mut state);
        deliver(&mut state, "b.rs", 1, Ok(sample("b.rs", &["stale"])));
        assert_eq!(state.preview.preview, PreviewStatus::Loading);
        assert!(doc(&state).is_none(), "a stale document reached the pane");
    }

    #[test]
    fn a_failed_load_reaches_the_status_bar_in_english() {
        let mut state = with_file();
        let mut effects = sync_target(&mut state);
        let Effect::LoadPreview { generation, .. } = effects.pop().expect("load") else {
            panic!("expected a preview load");
        };
        deliver(
            &mut state,
            "a.rs",
            generation,
            Err(PreviewError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such file",
            ))),
        );
        let message = state.visible_message().expect("message");
        assert!(message.text.contains("cannot preview"), "{message:?}");
        assert!(
            message.text.contains("not found"),
            "the operating system text leaked: {message:?}"
        );
    }

    /// A state whose preview already shows `lines` lines of `a.rs`.
    fn loaded(lines: usize) -> AppState {
        let mut state = with_file();
        let mut effects = sync_target(&mut state);
        let Effect::LoadPreview { generation, .. } = effects.pop().expect("a preview load") else {
            panic!("expected a preview load");
        };
        deliver(
            &mut state,
            "a.rs",
            generation,
            Ok(sample("a.rs", &vec!["x"; lines])),
        );
        assert_eq!(state.preview.preview, PreviewStatus::Ready);
        state
    }

    #[test]
    fn scrolling_only_happens_when_the_preview_has_focus() {
        let mut state = loaded(100);
        // Tree focus: the key belongs to the tree.
        state.focus = Focus::Tree;
        reduce(&mut state, &Action::Command(Command::ScrollDown));
        assert_eq!(state.preview.scroll_line, 0);
        state.focus = Focus::Preview;
        reduce(&mut state, &Action::Command(Command::ScrollDown));
        assert_eq!(state.preview.scroll_line, 1);
    }

    #[test]
    fn page_and_edge_commands_reach_both_ends() {
        let mut state = loaded(100);
        state.focus = Focus::Preview;
        reduce(&mut state, &Action::Command(Command::ScrollBottom));
        assert!(state.preview.at_bottom());
        reduce(&mut state, &Action::Command(Command::ScrollTop));
        assert_eq!(state.preview.scroll_line, 0);
        reduce(&mut state, &Action::Command(Command::ScrollPageDown));
        assert_eq!(state.preview.scroll_line, 10);
        reduce(&mut state, &Action::Command(Command::ScrollHalfPageDown));
        assert_eq!(state.preview.scroll_line, 15);
        reduce(&mut state, &Action::Command(Command::ScrollPageUp));
        assert_eq!(state.preview.scroll_line, 5);
    }

    #[test]
    fn sideways_scrolling_moves_the_columns() {
        let mut state = loaded(20);
        state.focus = Focus::Preview;
        // The lines are one column wide, so a pane of any size fits them.
        reduce(&mut state, &Action::Command(Command::ScrollRight));
        assert_eq!(state.preview.scroll_col, 0, "nothing to scroll to");
        let wide: Vec<String> = (0..20).map(|i| format!("line {i:0>40}")).collect();
        let generation = state.preview.generation;
        deliver(
            &mut state,
            "a.rs",
            generation,
            Ok(sample_owned("a.rs", wide)),
        );
        reduce(&mut state, &Action::Command(Command::ScrollRight));
        assert_eq!(state.preview.scroll_col, 1);
        // A page sideways stops where the widest line still fills the pane,
        // so the right edge of the document is not scrolled past.
        let width = body_width(&state);
        assert_eq!(state.preview.max_scroll_col(width), 3);
        reduce(&mut state, &Action::Command(Command::ScrollPageRight));
        assert_eq!(state.preview.scroll_col, 3);
        reduce(&mut state, &Action::Command(Command::ScrollPageLeft));
        assert_eq!(state.preview.scroll_col, 0);
        reduce(&mut state, &Action::Command(Command::ScrollLeft));
        assert_eq!(
            state.preview.scroll_col, 0,
            "cannot scroll before the first column"
        );
    }

    #[test]
    fn a_reload_keeps_the_generation_ahead_of_the_old_result() {
        let mut state = with_file();
        sync_target(&mut state);
        let effects = reload(&mut state);
        assert_eq!(load_effects(effects), vec![(PathBuf::from("a.rs"), 2)]);
        // A late result for generation 1 no longer applies.
        deliver(&mut state, "a.rs", 1, Ok(sample("a.rs", &["stale"])));
        assert!(doc(&state).is_none(), "a stale document reached the pane");
    }

    #[test]
    fn a_reload_keeps_the_body_on_screen_until_the_new_one_arrives() {
        let mut state = loaded(100);
        let shown = doc(&state).cloned().expect("a body");
        let generation = state.preview.generation;
        // The old body is still there, so the pane does not blink empty while
        // the file is being read again.
        let effects = reload(&mut state);
        assert_eq!(
            load_effects(effects),
            vec![(PathBuf::from("a.rs"), generation + 1)]
        );
        assert_eq!(doc(&state), Some(&shown));
        assert_eq!(state.preview.preview, PreviewStatus::Ready);
        // The scroll does not jump either.
        state.preview.scroll_line = 40;
        reload(&mut state);
        assert_eq!(state.preview.scroll_line, 40);
    }

    #[test]
    fn a_reload_works_from_the_tree_as_well() {
        let mut state = loaded(10);
        state.focus = Focus::Tree;
        let effects = reduce(&mut state, &Action::Command(Command::Reload));
        assert_eq!(
            load_effects(effects),
            vec![(PathBuf::from("a.rs"), state.preview.generation)]
        );
    }

    /// A state whose preview shows a diff with a hunk near each end.
    fn diff_loaded() -> (AppState, Vec<usize>) {
        use crate::app::state::PreviewMode;
        use crate::core::diff::Diff;

        let baseline = (0..40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut changed: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        changed[2] = "changed early".to_owned();
        changed[35] = "changed late".to_owned();
        let diff = Diff::compute(&baseline, &changed.join("\n")).expect("a diff");
        let hunks = diff.hunks.clone();
        assert!(
            hunks.len() >= 2,
            "the fixture needs several hunks: {hunks:?}"
        );

        let mut state = state();
        state.preview_mode = PreviewMode::InlineDiff;
        // A one line pane keeps every hunk scrollable to, so the test is
        // about picking hunks rather than about the height of the view.
        state.preview.viewport = 1;
        let mut doc = sample("a.rs", &[]);
        doc.lines = (0..diff.rows.len())
            .map(|row| format!("row {row}"))
            .collect();
        doc.diff = Some(diff);
        state.preview.set_doc(doc);
        (state, hunks)
    }

    #[test]
    fn n_and_n_step_through_the_hunks_of_the_diff() {
        let (mut state, hunks) = diff_loaded();
        // From the top, the first press lands on the first hunk.
        reduce(&mut state, &Action::Command(Command::NextHunk));
        assert_eq!(state.preview.scroll_line, hunks[0]);
        reduce(&mut state, &Action::Command(Command::NextHunk));
        assert_eq!(state.preview.scroll_line, hunks[1]);
        // The last hunk is as far as the key goes.
        reduce(&mut state, &Action::Command(Command::NextHunk));
        assert_eq!(state.preview.scroll_line, hunks[1]);
        reduce(&mut state, &Action::Command(Command::PrevHunk));
        assert_eq!(state.preview.scroll_line, hunks[0]);
        // And there is no hunk above the first one.
        reduce(&mut state, &Action::Command(Command::PrevHunk));
        assert_eq!(state.preview.scroll_line, hunks[0]);
    }

    #[test]
    fn hunk_jumping_works_from_the_tree_and_only_in_a_diff() {
        let (mut state, hunks) = diff_loaded();
        state.focus = Focus::Tree;
        reduce(&mut state, &Action::Command(Command::NextHunk));
        assert_eq!(state.preview.scroll_line, hunks[0]);

        // The content mode has no hunks to jump to.
        state.preview_mode = crate::app::state::PreviewMode::Content;
        state.preview.scroll_line = 0;
        reduce(&mut state, &Action::Command(Command::NextHunk));
        assert_eq!(state.preview.scroll_line, 0);

        // And neither has a plain document in a diff mode.
        state.preview_mode = crate::app::state::PreviewMode::InlineDiff;
        let plain = sample("a.rs", &["only", "context"]);
        state.preview.set_doc(plain);
        reduce(&mut state, &Action::Command(Command::NextHunk));
        assert_eq!(state.preview.scroll_line, 0);
    }

    #[test]
    fn a_reload_without_a_file_asks_for_nothing() {
        let mut state = state();
        assert!(reduce(&mut state, &Action::Command(Command::Reload)).is_empty());
    }

    #[test]
    fn a_load_carries_the_mode_and_the_baseline_build() {
        use crate::app::state::PreviewMode;

        let mut state = with_file();
        state.preview_mode = PreviewMode::InlineDiff;
        state.baseline.ver = 7;
        let effects = sync_target(&mut state);
        match effects.into_iter().next().expect("a load") {
            Effect::LoadPreview {
                mode, baseline_ver, ..
            } => {
                assert_eq!(mode, PreviewMode::InlineDiff);
                assert_eq!(baseline_ver, 7);
            }
            other => panic!("expected a preview load, got {other:?}"),
        }
    }
}
