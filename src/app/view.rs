//! Drawing a whole frame.

use ratatui::Frame;

use crate::app::state::{AppState, Overlay};
use crate::features::{finder, help, preview, statusbar, tree, watch};
use crate::ui::layout;

/// Draws one frame. Reads the state only, never writes it (invariant I7).
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    if layout::too_small(area) {
        crate::ui::render_too_small(frame, area, &state.theme);
        return;
    }
    let areas = layout::compute(area, state.tree_visible);
    if state.tree_visible {
        tree::view::render(frame, areas.tree, state);
    }
    preview::view::render_header(frame, areas.header, state);
    preview::view::render_body(frame, areas.preview, state);
    statusbar::view::render(frame, areas.status, state);
    // Every overlay draws over the panes, which stay readable around the
    // status bar (§10).
    match state.overlay {
        Overlay::None => {}
        Overlay::Files | Overlay::Grep => finder::view::render(frame, area, state),
        Overlay::Help => help::view::render(frame, area, state),
        Overlay::Watch => watch::view::render(frame, area, state),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::state::Config;
    use crate::cli::{BaselineKind, IconStyle};

    fn state(width: u16, height: u16, no_color: bool) -> AppState {
        AppState::new(
            Config {
                root: PathBuf::from("/root"),
                icons: IconStyle::Text,
                baseline: BaselineKind::Auto,
                ignore_enabled: true,
                no_color,
                log: None,
                selftest_panic: false,
            },
            (width, height),
        )
    }

    fn text(state: &AppState) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(state.size.0, state.size.1)).expect("terminal");
        terminal.draw(|frame| render(frame, state)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    #[test]
    fn a_small_terminal_only_says_so() {
        let text = text(&state(40, 10, false));
        assert!(text.contains("terminal too small"), "{text}");
        assert!(!text.contains("NORMAL"), "{text}");
    }

    #[test]
    fn the_minimum_size_is_enough_for_the_whole_layout() {
        let text = text(&state(60, 15, false));
        assert!(!text.contains("too small"), "{text}");
        assert!(text.contains("NORMAL"), "{text}");
    }

    #[test]
    fn hiding_the_tree_gives_the_width_to_the_preview() {
        let mut state = state(80, 24, false);
        let with_tree = text(&state);
        state.tree_visible = false;
        let without_tree = text(&state);
        assert!(with_tree.contains("│"), "{with_tree}");
        assert!(!without_tree.contains('│'), "{without_tree}");
    }

    #[test]
    fn the_help_overlay_draws_over_the_panes_and_keeps_the_status_bar() {
        let mut state = state(80, 24, false);
        crate::app::reduce::reduce(
            &mut state,
            crate::app::Action::Command(crate::app::command::Command::OpenHelp),
        );
        let text = text(&state);
        assert!(text.contains("HELP"), "{text}");
        assert!(text.contains("Everywhere"), "{text}");
        assert!(text.contains("Quit"), "{text}");
        let status = text.lines().last().unwrap_or_default();
        assert!(
            status.contains("HELP"),
            "the status bar is still there: {status}"
        );
    }

    #[test]
    fn the_watch_overlay_draws_the_log_it_has_kept() {
        let mut state = state(80, 24, false);
        crate::app::reduce::reduce(
            &mut state,
            crate::app::Action::FsBatch {
                changes: vec![crate::core::fswatch::FsChange {
                    rel: PathBuf::from("a.txt"),
                    kind: crate::core::fswatch::ChangeKind::Modify,
                }],
                at: std::time::SystemTime::UNIX_EPOCH,
            },
        );
        crate::app::reduce::reduce(
            &mut state,
            crate::app::Action::Command(crate::app::command::Command::OpenWatch),
        );
        let text = text(&state);
        assert!(text.contains("WATCH"), "{text}");
        assert!(text.contains("a.txt"), "{text}");
        let status = text.lines().last().unwrap_or_default();
        assert!(
            status.contains("WATCH"),
            "the status bar is still there: {status}"
        );
    }

    #[test]
    fn an_overlay_hides_everything_it_covers() {
        use crate::core::fsmodel::{FsModel, NodeKind};
        use crate::core::preview::{Doc, Language};

        let mut state = state(80, 24, false);
        state.preview.viewport = 20;
        // A tree with a selected file, so opening the log keeps the document
        // the pane is showing instead of clearing it.
        let mut model = FsModel::new(PathBuf::from("/root"));
        let root = model.root_id();
        let file = model.insert_child(root, std::ffi::OsStr::new("a.txt"), NodeKind::File);
        state.tree.model = model;
        state.tree.select(file);
        let doc = Doc {
            rel: PathBuf::from("a.txt"),
            name: "a.txt".into(),
            size: 64,
            language: Language::Text,
            lines: std::iter::once("first row, above the popup".to_owned())
                .chain((2..=10).map(|i| format!("ROW_{i}_BEHIND_THE_OVERLAY")))
                .collect(),
            spans: Vec::new(),
            truncated: false,
            too_large: false,
            highlight_truncated: false,
            invalid_utf8: false,
            binary: None,
            link_target: None,
            diff: None,
            diff_note: None,
            formatted: false,
        };
        state.preview.path = Some(doc.rel.clone());
        state.preview.set_doc(doc);
        // The rows after the first fall where the popup is about to stand.
        assert!(text(&state).contains("ROW_5_BEHIND_THE_OVERLAY"));
        crate::app::reduce::reduce(
            &mut state,
            crate::app::Action::Command(crate::app::command::Command::OpenWatch),
        );
        let text = text(&state);
        assert!(text.contains("WATCH"), "{text}");
        assert!(
            !text.contains("BEHIND_THE_OVERLAY"),
            "the panes behind the overlay show through: {text}"
        );
    }

    /// The frame colours at the top left corner of the tree and of the
    /// preview header.
    fn frame_corners(state: &AppState) -> (ratatui::style::Color, ratatui::style::Color) {
        let mut terminal =
            Terminal::new(TestBackend::new(state.size.0, state.size.1)).expect("terminal");
        terminal.draw(|frame| render(frame, state)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let areas = crate::ui::layout::compute(
            ratatui::layout::Rect::new(0, 0, state.size.0, state.size.1),
            state.tree_visible,
        );
        (
            buffer[(0, 0)].fg,
            buffer[(areas.header.x, areas.header.y)].fg,
        )
    }

    #[test]
    fn the_frame_of_the_focused_pane_is_the_bright_one() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::style::Color;

        let mut state = state(80, 24, false);
        let (tree, header) = frame_corners(&state);
        assert_eq!(tree, Color::White, "the tree holds the focus");
        assert_eq!(header, Color::DarkGray, "the preview does not");

        crate::app::reduce::reduce(
            &mut state,
            crate::app::Action::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        );
        let (tree, header) = frame_corners(&state);
        assert_eq!(tree, Color::DarkGray, "the focus moved away");
        assert_eq!(header, Color::White, "the preview holds the focus now");
    }
}
